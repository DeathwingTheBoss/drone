use actix_cors::Cors;
use actix_web::{web, App, HttpRequest, HttpResponse, HttpServer, Responder};
use async_recursion::async_recursion;
use config::Config;
use moka::sync::Cache;
use reqwest::{Client, ClientBuilder};
use serde::{Deserialize, Serialize, Serializer};
use serde_json::Value;
use serde_with::{serde_as, DurationSeconds};
use std::{sync::Mutex, time::Duration, fmt::Display};

const DRONE_VERSION: &str = env!("CARGO_PKG_VERSION");

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

// Enum for API Requests, either single or batch.
#[derive(Deserialize, Debug)]
#[serde(untagged)]
enum APICall {
    Single(APIRequest),
    Batch(Vec<APIRequest>),
}

// Enum for id in JSONRPC body.
#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(untagged)]
enum ID {
    Str(String),
    Int(u32),
}


// Structure for API calls.
#[derive(Serialize, Deserialize, Debug)]
struct APIRequest {
    jsonrpc: String,
    id: ID,
    method: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    params: Option<Value>,
}

#[derive(Debug, Deserialize)]
enum ErrorField {
    Object(Value),   // JSON from Hived
    Message(String), // Custom message
}

impl Serialize for ErrorField {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match self {
            ErrorField::Object(json_value) => json_value.serialize(serializer),
            ErrorField::Message(text) => text.serialize(serializer),
        }
    }
}

// Structure for the error response.
#[derive(Serialize, Deserialize, Debug)]
struct ErrorStructure {
    jsonrpc: String,
    id: ID,
    code: i32,
    message: String,
    error: ErrorField,
}

// Enum for the endpoints.
enum Endpoints {
    HAF,
    HAFAH,
    HIVEMIND,
}

struct APICallResponse {
    jsonrpc: String,
    result: Value,
    id: ID,
    cached: bool,
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

impl Display for Endpoints {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Endpoints::HAF => write!(f, " Endpoint: HAF"),
            Endpoints::HAFAH => write!(f, " Endpoint: HAFAH"),
            Endpoints::HIVEMIND => write!(f, " Endpoint: HIVEMIND"),
        }
    }
}

#[async_recursion]
async fn handle_request(
    request: &APIRequest,
    data: &web::Data<AppData>,
    client_ip: &String,
) -> Result<APICallResponse, ErrorStructure> {
    // Convert the call to a struct.
    let client = data.webclient.clone();
    let method = request.method.as_str();

    if method == "call" {
        if let Some(params) = request.params.as_ref().and_then(Value::as_array) {
            if params.len() > 2 {
                if let Some(method) = params[0].as_str() {
                    let format_new_method = format!("{}.{}", method, params[1].as_str().unwrap());
                    let new_params = params[2].clone();
                    let new_request = APIRequest {
                        jsonrpc: "2.0".to_string(),
                        id: request.id.clone(),
                        method: format_new_method,
                        params: Some(new_params),
                    };
                    return handle_request(&new_request, data, client_ip).await;
                }
            }
        }
    }

    // Get humantime for logging.
    let human_timestamp = humantime::format_rfc3339_seconds(std::time::SystemTime::now());
    let formatted_log = if let Some(params) = &request.params {
        format!(
            "Timestamp: {} || IP: {} || Request Method: {} || Request Params: {}",
            human_timestamp, client_ip, request.method, params,
        )
    } else {
        format!(
            "Timestamp: {} || IP: {} || Request Method: {}",
            human_timestamp, client_ip, request.method,
        )
    };
    println!("{}", formatted_log);


    // Pick the endpoints depending on the method.
    let endpoints = match method {
        // HAF
        "condenser_api.get_block" => Endpoints::HAF,
        "block_api.get_block_range" => Endpoints::HAF,
        "condenser_api.get_block_range" => Endpoints::HAF,
        // HAFAH
        _ if method.starts_with("account_history_api.") => Endpoints::HAFAH,
        "condenser_api.get_ops_in_block" => Endpoints::HAFAH,
        "condenser_api.get_transaction" => Endpoints::HAFAH,
        "condenser_api.get_account_history" => Endpoints::HAFAH,
        "database_api.get_account_history" => Endpoints::HAFAH,
        // HIVEMIND
        _hive_endpoint if method.starts_with("hive.") => Endpoints::HIVEMIND,
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
        "database_api.find_comments" => Endpoints::HIVEMIND,
        _bridge_endpoint if method.starts_with("bridge.") => Endpoints::HIVEMIND,
        _follow_api_endpoint if method.starts_with("follow_api.") => Endpoints::HIVEMIND, // Remove when beem is updated. (Deprecated)
        _tags_api_endpoint if method.starts_with("tags_api.") => Endpoints::HIVEMIND, // Remove when beem is updated. (Deprecated)
        _anything_else => Endpoints::HAF,
    };

    // Check if the call is in the cache. If it is, return only the result while keeping the rest of the response the same.
    let params_str = request.params.as_ref().map_or("[]".to_string(), |v: &Value| v.to_string());
    if let Some(cached_call) = data.cache.get(&params_str) {
        // build result with data from cache and response
        let result = cached_call.clone();
        return Ok(APICallResponse {
            jsonrpc: request.jsonrpc.clone(),
            result: result["result"].clone(),
            id: request.id.clone(),
            cached: true,
        });
    }


    // Send the request to the endpoints.
    let res = match client
        .post(endpoints.choose_endpoint(&data))
        .json(&request)
        .send()
        .await
    {
        Ok(response) => response,
        Err(err) => {
            let mut error_message = err.without_url().to_string();
            error_message.push_str(&endpoints.to_string());
            return Err(ErrorStructure {
                jsonrpc: request.jsonrpc.clone(),
                id : request.id.clone(),
                code: -32700,
                message: format!("Unable to send request to endpoint."),
                error: ErrorField::Message(error_message),
            })
        }
    };
    let body = match res.text().await {
        Ok(text) => text,
        Err(err) => {
            return Err(ErrorStructure {
                jsonrpc: request.jsonrpc.clone(),
                id : request.id.clone(),
                code: -32600,
                message: format!("Received an invalid response from the endpoint."),
                error: ErrorField::Message(err.to_string()),
            })
        }
    };
    let json_body: serde_json::Value = match serde_json::from_str(&body) {
        Ok(parsed) => parsed,
        Err(err) => {
            return Err(ErrorStructure {
                jsonrpc: request.jsonrpc.clone(),
                id : request.id.clone(),
                code: -32602,
                message: format!("Unable to parse endpoint data."),
                error: ErrorField::Message(err.to_string()),
            })
        }
    };
    if json_body["error"].is_object() {
        return Err(ErrorStructure {
            jsonrpc: request.jsonrpc.clone(),
            id : request.id.clone(),
            code: -32700,
            message: format!("Endpoint returned an error."),
            error: ErrorField::Object(json_body["error"].clone()),
        });
    }
    let mut cacheable = true;
    if json_body["result"].is_array() && json_body["result"].as_array().unwrap().is_empty()
        || json_body["result"].is_null()
        || json_body["result"]["blocks"].is_array()
            && json_body["result"]["blocks"].as_array().unwrap().is_empty()
    {
        cacheable = false;
    }

    if DRONE_CACHEABLE_METHODS.contains(&request.method.as_str()) && cacheable {
        let params_str = request.params.as_ref().map_or("[]".to_string(), |v| v.to_string());
        data.cache.insert(params_str, json_body.clone());
    }
    Ok(APICallResponse {
        jsonrpc: request.jsonrpc.clone(),
        result: json_body["result"].clone(),
        id: request.id.clone(),
        cached: false,
    })
}

async fn api_call(
    req: HttpRequest,
    call: web::Json<APICall>,
    data: web::Data<AppData>,
) -> impl Responder {
    let get_cloudflare_ip = req.headers().get("CF-Connecting-IP");

    let client_ip = match get_cloudflare_ip {
        Some(ip) => ip.to_str().map(|ip| ip.to_string()),
        None => Ok(req.peer_addr().unwrap().ip().to_string()),
    };
    let user_ip = match client_ip {
        Ok(ip) => ip,
        Err(_) => {
            return HttpResponse::InternalServerError().json(ErrorStructure {
                jsonrpc: "2.0".to_string(),
                id: ID::Int(0),
                code: -32000,
                message: "Internal Server Error".to_string(),
                error: ErrorField::Message("Invalid Cloudflare Proxy Header.".to_string()),
            })
        }
    };

    match call.0 {
        APICall::Single(request) => {
            let result = handle_request(&request, &data, &user_ip).await;
            match result {
                Ok(response) => HttpResponse::Ok()
                    .insert_header(("Drone-Version", DRONE_VERSION))
                    .insert_header(("Cache-Status", response.cached.to_string()))
                    .json(serde_json::json!({
                        "jsonrpc": response.jsonrpc,
                        "result": response.result,
                        "id": response.id,
                    })),
                Err(err) => HttpResponse::InternalServerError().json(err),
            }
        }
        APICall::Batch(requests) => {
            let mut responses = Vec::new();
            if requests.len() > 100 {
                return HttpResponse::InternalServerError().json(ErrorStructure {
                    jsonrpc: "2.0".to_string(),
                    id: ID::Int(0),
                    code: -32600,
                    message: "Request parameter error.".to_string(),
                    error: ErrorField::Message(
                        "Batch size too large, maximum allowed is 100.".to_string(),
                    ),
                });
            }

            for request in requests {
                let result = handle_request(&request, &data, &user_ip).await;
                match result {
                    Ok(response) => responses.push(response),
                    Err(err) => return HttpResponse::InternalServerError().json(err),
                }
            }
            let mut cached = true;
            let mut result = Vec::new();
            for response in responses {
                if response.cached == false {
                    cached = false;
                }
                result.push(serde_json::json!({
                    "jsonrpc": response.jsonrpc,
                    "result": response.result,
                    "id": response.id,
                }));
            }
            HttpResponse::Ok()
                .insert_header(("Drone-Version", DRONE_VERSION))
                .insert_header(("Cache-Status", cached.to_string()))
                .json(serde_json::Value::Array(result))
        }
    }
}

struct AppData {
    cache: Cache<String, Value>,
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
    cache_count: u64,
    operator_message: String,
    haf_endpoint: String,
    hafah_endpoint: String,
    hivemind_endpoint: String,
    middleware_connection_threads: usize,
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
        cache: Cache::builder()
            .max_capacity(config.cache_count)
            .time_to_live(config.cache_ttl)
            .build(),
        webclient: ClientBuilder::new()
            .pool_max_idle_per_host(config.middleware_connection_threads)
            .build()
            .unwrap(),
        config: config.clone(),
    });
    println!("Drone is running on port {}.", config.port);
    HttpServer::new(move || {
        let cors = Cors::permissive();
        App::new()
            .wrap(cors)
            .app_data(
                web::JsonConfig::default()
                    .content_type(|_| true)
                    .content_type_required(false)
                    .limit(1024 * 100),
            ) // 100kb
            .app_data(_cache.clone())
            .route("/", web::get().to(index))
            .route("/", web::post().to(api_call))
            .route("/health", web::get().to(index))
    })
    .bind((config.hostname, config.port))?
    .run()
    .await
}
