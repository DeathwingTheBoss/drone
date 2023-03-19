use std::{time::Duration, sync::Mutex};
use actix_web::{get, post, web, App, HttpResponse, HttpServer, Responder, HttpRequest};
use serde::{Deserialize, Serialize};
use reqwest::{Client, ClientBuilder};
use lru_time_cache::LruCache;
use serde_json::Value;

// Drone Configuration Parameters
const DRONE_VERSION: &str = env!("CARGO_PKG_VERSION");
const DRONE_PORT: u16 = 8999;
const DRONE_HOST: &str = "0.0.0.0";
const DRONE_CACHE_TTL: Duration = Duration::from_secs(300);
const DRONE_CACHE_COUNT: usize = 250;
const DRONE_OPERATOR_MESSAGE: &str = "Drone by Deathwing";
const DRONE_HAF_ENDPOINT_IP: &str = "http://HAFIP:PORT"; // Set this to your HAF endpoint. (http://HAFIP:PORT)
const DRONE_HAFAH_ENDPOINT_IP: &str = "http://HAFAHIP:PORT"; // Set this to your HAFAH endpoint. (http://HAFAHIP:PORT)
const DRONE_HIVEMIND_ENDPOINT_IP: &str = "http://HIVEMINDIP:PORT"; // Set this to your HIVEMIND endpoint. (http://HIVEMINDIP:PORT)

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

#[get("/")]
async fn index() -> impl Responder {
    // Reply with health check JSON.
    HttpResponse::Ok().json(HealthCheck {
        status: "OK".to_string(),
        drone_version: DRONE_VERSION.to_string(),
        message: DRONE_OPERATOR_MESSAGE.to_string(),
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
    fn choose_endpoint(&self) -> &'static str {
        match self {
            Endpoints::HAF => DRONE_HAF_ENDPOINT_IP,
            Endpoints::HAFAH => DRONE_HAFAH_ENDPOINT_IP,
            Endpoints::HIVEMIND => DRONE_HIVEMIND_ENDPOINT_IP,
        }
    }
}

#[post("/")]
async fn api_call(req: HttpRequest, call: web::Json<APICall>, data: web::Data<APIFunctions>) -> impl Responder {

    // Convert the call to a struct.
    let call = call.into_inner();
    let client = data.webclient.clone();
    let json_rpc_call = APICall {
        jsonrpc: "2.0".to_string(),
        id: call.id,
        method: call.method,
        params: call.params,
    };

    // Print request details.
    let formatted_log = 
    format!("IP: {} || HTTP Version: {:?} || Request Method: {} || Request Params: {}", req.peer_addr().unwrap(), req.version(), json_rpc_call.method, json_rpc_call.params);
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
    if let Some(cached_call) = data.cache.lock().unwrap().get(&json_rpc_call.params.to_string()) {
        cache_status = "HIT";
        return HttpResponse::Ok()
        .insert_header(("Drone-Version", DRONE_VERSION))
        .insert_header(("Cache-Status", cache_status))
        .json(cached_call);
    }

    // Send the request to the endpoints.
    let res = match client.post(endpoints.choose_endpoint())
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
    if json_body["result"].is_array() && json_body["result"].as_array().unwrap().is_empty() || json_body["result"].is_null() {
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



struct APIFunctions {
    cache: Mutex<LruCache<String, Value>>,
    webclient: Client,
}


#[actix_web::main]
async fn main() -> std::io::Result<()> {
    // Create the cache.
    let _cache = web::Data::new(APIFunctions {
        cache: Mutex::new(LruCache::<String, serde_json::Value>::with_expiry_duration_and_capacity(DRONE_CACHE_TTL, DRONE_CACHE_COUNT)),
        webclient: ClientBuilder::new()
            .pool_max_idle_per_host(8)
            .build()
            .unwrap(),
    });
    HttpServer::new(move || {
        App::new()
            .app_data(_cache.clone())
            .service(index)
            .service(api_call)
    })
    .bind((DRONE_HOST, DRONE_PORT))?
    .run()
    .await
}