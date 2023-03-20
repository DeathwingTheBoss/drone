use std::{time::Duration, sync::Mutex};
use actix_web::{web::{self, JsonConfig}, App, HttpResponse, HttpServer, Responder, HttpRequest};
use serde::{Deserialize, Serialize};
use reqwest::{Client, ClientBuilder};
use lru_time_cache::LruCache;
use serde_json::Value;
use serde_with::{DurationSeconds, serde_as};
use config::Config;

const DRONE_VERSION: &str  = env!("CARGO_PKG_VERSION");

// Drone Cacheable Methods
const DRONE_CACHEABLE_METHODS: [&str; 7] = [
    "block_api.get_block",
    "condenser_api.get_block",
    "account_history_api.get_transaction",
    "condenser_api.get_transaction",
    "condenser_api.get_ops_in_block",
    "condenser_api.get_block_range",
    "block_api.get_block_range",
];


#[derive(Serialize, Deserialize, Debug)]
struct HealthCheck {
    status: String,
    drone_version: String,
    message: String,
}

// Use Index for both / and /health.
async fn index(appdata: web::Data<AppData>) -> impl Responder {
    // Reply with health check JSON.
    HttpResponse::Ok().json(HealthCheck {
        status: "OK".to_string(),
        drone_version: DRONE_VERSION.to_string(),
        message: appdata.config.operator_message.to_string(),
    })
}

// Structure for API calls.
#[derive(Serialize, Deserialize, Debug)]
struct APICall {
    jsonrpc: String,
    id: u32,
    method: String,
    params: Value,
}

// Structure for the error response.
#[derive(Serialize, Deserialize, Debug)]
struct ErrorStructure {
    code: i32,
    message: String,
    error_data: String,
}

// Enum for the endpoints.
enum Endpoints {
    HAF,
    HAFAH,
    HIVEMIND,
}

// Choose the endpoint depending on the method.
impl Endpoints {
    fn choose_endpoint<'a>(&self, appdata: &'a web::Data<AppData>) -> &'a str {
        match self {
            Endpoints::HAF => appdata.config.haf_endpoint.as_str(),
            Endpoints::HAFAH => appdata.config.hafah_endpoint.as_str(),
            Endpoints::HIVEMIND => appdata.config.hivemind_endpoint.as_str(),
        }
    }
}

async fn api_call(req: HttpRequest, call: web::Json<APICall>, data: web::Data<AppData>) -> impl Responder {

    // Convert the call to a struct.
    let call = call.into_inner();
    let client = data.webclient.clone();
    let json_rpc_call = APICall {
        jsonrpc: "2.0".to_string(),
        id: call.id,
        method: call.method,
        params: call.params,
    };

    // Get humantime for logging.
    let human_timestamp = humantime::format_rfc3339_seconds(std::time::SystemTime::now());

    // Log the request, if there's Cloudflare header (CF-Connecting-IP) use that instead of peer_addr.
    let get_cloudflare_ip = req.headers().get("CF-Connecting-IP");

    let client_ip = match get_cloudflare_ip {
        Some(ip) => ip.to_str().map(|ip| ip.to_string()),
        None => Ok(req.peer_addr().unwrap().ip().to_string()),
    };
    let client_ip = match client_ip {
        Ok(ip) => ip,
        Err(_) => return HttpResponse::InternalServerError().json(ErrorStructure {
            code: 9999,
            message: "Internal Server Error".to_string(),
            error_data: "Invalid Cloudflare Proxy Header.".to_string(),
        }),
    };
    
    let formatted_log = 
    format!(
        "Timestamp: {} || IP: {} || Request Method: {} || Request Params: {}",
        human_timestamp,
        client_ip, 
        json_rpc_call.method, 
        json_rpc_call.params,
    );
    println!("{}", formatted_log);
    // Pick the endpoints depending on the method.
    let endpoints = match json_rpc_call.method.as_str() {
        // HAF
        "condenser_api.get_block" => Endpoints::HAF,
        "block_api.get_block_range" => Endpoints::HAF,
        "condenser_api.get_block_range" => Endpoints::HAF,
        // HAFAH
        "account_history_api" => Endpoints::HAFAH,
        "account_history_api.get_ops_in_block" => Endpoints::HAFAH,
        "account_history_api.enum_virtual_ops" => Endpoints::HAFAH,
        "account_history_api.get_transaction" => Endpoints::HAFAH,
        "account_history_api.get_account_history" => Endpoints::HAFAH,
        "condenser_api.get_ops_in_block" => Endpoints::HAFAH,
        "condenser_api.get_transaction" => Endpoints::HAFAH,
        "condenser_api.get_account_history" => Endpoints::HAFAH,
        "database_api.get_account_history" => Endpoints::HAFAH,
        // HIVEMIND
        "condenser_api.get_content_replies" => Endpoints::HIVEMIND,
        "condenser_api.get_account_votes" => Endpoints::HIVEMIND,
        "condenser_api.get_followers" => Endpoints::HIVEMIND,
        "condenser_api.get_following" => Endpoints::HIVEMIND,
        "condenser_api.get_follow_count" => Endpoints::HIVEMIND,
        "condenser_api.get_blog" => Endpoints::HIVEMIND,
        "condenser_api.get_content" => Endpoints::HIVEMIND,
        "condenser_api.get_blog_entries" => Endpoints::HIVEMIND,
        "condenser_api.get_active_votes" => Endpoints::HIVEMIND,
        "condenser_api.get_discussions_by_trending" => Endpoints::HIVEMIND,
        "condenser_api.get_discussions_by_hot" => Endpoints::HIVEMIND,
        "condenser_api.get_discussions_by_promoted" => Endpoints::HIVEMIND,
        "condenser_api.get_discussions_by_created" => Endpoints::HIVEMIND,
        "condenser_api.get_discussions_by_blog" => Endpoints::HIVEMIND,
        "condenser_api.get_discussions_by_feed" => Endpoints::HIVEMIND,
        "condenser_api.get_discussions_by_comments" => Endpoints::HIVEMIND,
        "condenser_api.get_discussions_by_author_before_date" => Endpoints::HIVEMIND,
        "condenser_api.get_state" => Endpoints::HIVEMIND,
        "condenser_api.get_trending_tags" => Endpoints::HIVEMIND,
        "condenser_api.get_post_discussions_by_payout" => Endpoints::HIVEMIND,
        "condenser_api.get_comment_discussions_by_payout" => Endpoints::HIVEMIND,
        "condenser_api.get_replies_by_last_update" => Endpoints::HIVEMIND,
        "condenser_api.get_reblogged_by" => Endpoints::HIVEMIND,
        "follow_api" => Endpoints::HIVEMIND,
        "tags_api" => Endpoints::HIVEMIND,
        _anything_else => Endpoints::HAF,
    };

    // Cache Status
    let mut cache_status = "MISS";
    // Check if the call is in the cache.
    if let Some(cached_call) = 
    data.cache.lock().unwrap().get(&json_rpc_call.params.to_string()) {
        cache_status = "HIT";
        return HttpResponse::Ok()
        .insert_header(("Drone-Version", DRONE_VERSION))
        .insert_header(("Cache-Status", cache_status))
        .json(cached_call);
    }

    // Send the request to the endpoints.
    let res = match client.post(endpoints.choose_endpoint(&data))
        .json(&json_rpc_call)
        .send()
        .await {
            Ok(response) => response,
            Err(err) => return HttpResponse::InternalServerError().json(ErrorStructure {
                code: 1000,
                message: format!("Unable to send the request to the endpoints."),
                error_data: err.to_string(),
            }),
        };
    let body = match res.text().await {
        Ok(text) => text,
        Err(err) => return HttpResponse::InternalServerError().json(ErrorStructure {
            code: 2000,
            message: format!("Received an invalid response from the endpoints."),
            error_data: err.to_string(),
        }),
    };
    let json_body: serde_json::Value = match serde_json::from_str(&body) {
        Ok(parsed) => parsed,
        Err(err) => return HttpResponse::InternalServerError().json(ErrorStructure {
            code: 3000,
            message: format!("Unable to parse endpoints data."),
            error_data: err.to_string(),
        }),
    };
    if json_body["result"].is_array() && 
       json_body["result"].as_array().unwrap().is_empty() || json_body["result"].is_null() {
        return HttpResponse::InternalServerError().json(ErrorStructure {
            code: 4000,
            message: format!("Unable to parse endpoints data."),
            error_data: "The endpoint returned an empty result.".to_string(),
        });
    }

    if DRONE_CACHEABLE_METHODS.contains(&json_rpc_call.method.as_str()) {
        // Insert the call in the cache.
        data.cache.lock().unwrap().insert(json_rpc_call.params.to_string(), json_body);
    }
    HttpResponse::Ok()
    .insert_header(("Drone-Version", DRONE_VERSION))
    .insert_header(("Cache-Status", cache_status))
    .body(body)
}



struct AppData {
    cache: Mutex<LruCache<String, Value>>,
    webclient: Client,
    config: DroneConfig,
}

#[serde_as]
#[derive(Clone, Deserialize, Debug)]
#[serde(rename_all = "UPPERCASE")]
struct DroneConfig {
    port: u16,
    hostname: String,
    #[serde_as(as = "DurationSeconds<u64>")]
    cache_ttl: Duration,
    cache_count: usize,
    operator_message: String,
    haf_endpoint: String,
    hafah_endpoint: String,
    hivemind_endpoint: String,
    actix_connection_threads: usize,
}


#[actix_web::main]
async fn main() -> std::io::Result<()> {
    // Load config.
    let config = Config::builder()
    .add_source(config::File::with_name("config.json"))
    .build()
    .expect("Could not load config.json")
    .try_deserialize::<DroneConfig>()
    .expect("config.json is not in a valid format.");
    
    // Create the cache.
    let _cache = web::Data::new(AppData {
        cache: Mutex::new(
            LruCache::<String, serde_json::Value>
            ::with_expiry_duration_and_capacity(config.cache_ttl, config.cache_count)),
        webclient: ClientBuilder::new()
            .pool_max_idle_per_host(config.actix_connection_threads)
            .build()
            .unwrap(),
        config: config.clone(),
    });
    println!("Drone is running on port {}.", config.port);
    HttpServer::new(move || {
        App::new()
            .app_data(_cache.clone())
            .app_data(JsonConfig::default().content_type_required(false))
            .route("/", web::get().to(index))
            .route("/", web::post().to(api_call))
            .route("/health", web::get().to(index))
    })
    .bind((config.hostname, config.port))?
    .run()
    .await
    
}