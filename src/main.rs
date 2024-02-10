use actix_cors::Cors;
use actix_web::{web, App, HttpRequest, HttpResponse, HttpServer, Responder};
use moka::{sync::Cache, Expiry};
use reqwest::{Client, ClientBuilder};
use serde::{Deserialize, Serialize, Serializer};
use serde_json::Value;
use std::sync::{Mutex};
use std::time::{Duration, Instant, SystemTime};
use std::collections::{HashMap};
use std::sync::atomic::{self, AtomicU32};
use actix_web::rt::time::sleep;
use futures_util::future::{FutureExt, Future,Shared};
use std::pin::Pin;
use std::sync::OnceLock;
use chrono::DateTime;



pub mod config;
use config::{AppConfig, TtlValue};

pub mod method_renamer;
use method_renamer::MethodAndParams;

const DRONE_VERSION: &str = env!("CARGO_PKG_VERSION");

// TODO: fix these, shouldn't be static globals
static last_irreversible_block_number: AtomicU32 = AtomicU32::new(0);
static current_head_block_number: AtomicU32 = AtomicU32::new(0);


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
        message: appdata.config.drone.operator_message.to_string(),
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
pub struct APIRequest {
    jsonrpc: String,
    id: ID,
    method: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    params: Option<Value>,
}

#[derive(Debug, Deserialize, Clone)]
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
#[derive(Serialize, Deserialize, Debug, Clone)]
struct ErrorStructure {
    jsonrpc: String,
    id: ID,
    code: i32,
    message: String,
    error: ErrorField,
}

#[derive(Clone)]
struct APICallResponse {
    /// the original value of jsonrpc request made by the caller (usually "2.0")
    jsonrpc: String,
    result: Value,
    /// the id the caller used in their request
    id: ID,
    // data returned just for logging/debugging
    cached: bool,
    mapped_method: MethodAndParams, // the method, parsed and transformed
    backend_url: Option<String>,
    upstream_method: Option<String>
}

#[derive(Clone, Debug)]
struct CacheEntry {
    result: Value,
    size: u32,
	ttl: TtlValue
}

pub struct MyExpiry;

impl Expiry<String, CacheEntry> for MyExpiry {
    /// Returns the duration of the expiration of the value that was just
    /// created.
    fn expire_after_create(&self, key: &String, value: &CacheEntry, _current_time: Instant) -> Option<Duration> {
		match value.ttl {
            TtlValue::NoExpire => { None }
			TtlValue::DurationInSeconds(s) => { 
                println!("MyExpiry: expire_after_create called with key {key}, returning duration {s}.");
                Some(Duration::from_secs(s as u64))
            }
			_ => { 
                println!("MyExpiry: expire_after_create called with key {key} that doesn't have a duration in seconds, this is unexpected.");
                None
            }
		}
    }
}

fn get_block_number_from_result(result: &Value) -> Option<u32> {
    // appbase get_block
    if let Some(block_num) = result.pointer("/block/block_id").and_then(|block_id| block_id.as_str()).and_then(|id_str| u32::from_str_radix(&id_str[..8], 16).ok()) {
        return Some(block_num);
    }
    // appbase get_block_header
    if let Some(prev_block_num) = result.pointer("/header/previous").and_then(|block_id| block_id.as_str()).and_then(|id_str| u32::from_str_radix(&id_str[..8], 16).ok()) {
        return Some(prev_block_num + 1);
    }
    // hived get_block
    if let Some(block_num) = result.pointer("/block_id").and_then(|block_id| block_id.as_str()).and_then(|id_str| u32::from_str_radix(&id_str[..8], 16).ok()) {
        return Some(block_num);
    }
    // hived get_block_header
    if let Some(prev_block_num) = result.pointer("/previous").and_then(|block_id| block_id.as_str()).and_then(|id_str| u32::from_str_radix(&id_str[..8], 16).ok()) {
        return Some(prev_block_num + 1);
    }

    None
}

// check a request to see if it's asking for a block that doesn't exist yet.  We get a lot of API
// calls that do this, presumably clients that are just polling for the next block.
// This is a case we can optimize.  Either by:
// - returning a stock error reply, without contacting the upstream, or
// - if the block is expected to arrive in a few seconds, just wait.  once the bloc arrives, return it
// Waiting seems better
fn check_for_future_block_requests(mapped_method: &MethodAndParams) {
    if mapped_method.method == "get_block" {
        if let Some(block_num) = mapped_method.params.as_ref().and_then(|v| v["block_num"].as_u64()) {
            if block_num as u32 > current_head_block_number.load(atomic::Ordering::Acquire) {
                // we're only testing against the head block number we recorded the last
                // time someone called get_dynamic_global_properties.
                // we should also check that now() is < the predicted time the requested
                // block will be produced
                println!("future block requested: {block_num}, head is {}", current_head_block_number.load(atomic::Ordering::Acquire));
            }
        }
    }
}

async fn request_from_upstream(request: APIRequest, data: web::Data<AppData>, mapped_method: MethodAndParams, method_and_params_str: String) -> Result<APICallResponse, ErrorStructure> {
    let endpoint = match data.config.lookup_url(mapped_method.get_method_name_parts()) {
        Some(endpoint) => { endpoint }
        None => {
            return Err(ErrorStructure {
                jsonrpc: request.jsonrpc.clone(),
                id : request.id.clone(),
                code: -32700,
                message: format!("Unable to map request to endpoint."),
                error: ErrorField::Message("Unable to map request to endpoint.".to_string()),
            });
        }
    };

    let upstream_request = mapped_method.format_for_upstream(&data.config);
    println!("Making upstream request using method {:?} and params {:?}", upstream_request.method, upstream_request.params);

    let client = data.webclient.clone();

    // Send the request to the endpoints.
    let res = match client
        .post(endpoint)
        .json(&upstream_request)
        .send()
        .await
    {
        Ok(response) => response,
        Err(err) => {
            let mut error_message = err.without_url().to_string();
            error_message.push_str(&endpoint.to_string());
            return Err(ErrorStructure {
                jsonrpc: request.jsonrpc.clone(),
                id : request.id.clone(),
                code: -32700,
                message: format!("Unable to send request to endpoint."),
                error: ErrorField::Message(error_message),
            })
        }
    };

    // to simulate slow calls, put a sleep here
    // sleep(Duration::from_secs(10)).await;

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

    // if the call was to get_dynamic_global_properties, save off the last irreversible block
    println!("Mapped method is {}", mapped_method.method);
    if mapped_method.method == "get_dynamic_global_properties" {
        println!("This method was get_dynamic_global_properties");
        if let Some(new_lib) = json_body["result"]["last_irreversible_block_num"].as_u64() {
            if new_lib as u32 > last_irreversible_block_number.load(atomic::Ordering::Acquire) {
                println!("new LIB is {new_lib}");
                last_irreversible_block_number.store(new_lib as u32, atomic::Ordering::Release);
            }
        }
        if let Some(new_head) = json_body["result"]["head_block_number"].as_u64() {
            if new_head as u32 > current_head_block_number.load(atomic::Ordering::Acquire) {
                println!("new head is {new_head}");
                current_head_block_number.store(new_head as u32, atomic::Ordering::Release);
                if let Some(new_time) = json_body["result"]["time"].as_str() {
                    let current_head_block_time = DateTime::parse_from_rfc3339(&format!("{new_time}Z")).unwrap();
                    let systemtime = SystemTime::from(current_head_block_time);
                    println!("Parsed datetime to {:?}, as systemtime: {:?}", current_head_block_time, systemtime);
                }
            }
        }
    }


    if json_body["result"].is_array() && json_body["result"].as_array().unwrap().is_empty()
        || json_body["result"].is_null()
        || json_body["result"]["blocks"].is_array() && json_body["result"]["blocks"].as_array().unwrap().is_empty()
    {
        // then this result shouldn't be cached
        // TODO: why?
    }
    else
    {
        let mut ttl = *data.config.lookup_ttl(mapped_method.get_method_name_parts()).unwrap_or(&TtlValue::NoCache);
        println!("Method lookup_ttl returns {ttl:?}");

        if ttl == TtlValue::ExpireIfReversible {
            // we cache forever if the block is irreversible, or 9 seconds if it's reversible
            if let Some(block_number) = get_block_number_from_result(&json_body["result"]) {
                ttl = if block_number > last_irreversible_block_number.load(atomic::Ordering::Acquire) { TtlValue::DurationInSeconds(9) } else { TtlValue::NoExpire };
                println!("block number from result is {block_number}, lib is {}, setting ttl to {:?}", last_irreversible_block_number.load(atomic::Ordering::Acquire), ttl);
            }
        }
        match ttl {
            TtlValue::NoExpire | TtlValue::DurationInSeconds(_) => {
                let entry = CacheEntry {
                    result: json_body["result"].clone(),
                    size: body.len() as u32,
                    ttl
                };
                data.cache.insert(method_and_params_str, entry);
            }
            _ => {}
        }
    }

    Ok(APICallResponse {
        jsonrpc: request.jsonrpc.clone(),
        result: json_body["result"].clone(),
        id: request.id.clone(),
        cached: false,
        mapped_method,
        backend_url: Some(endpoint.clone()),
        upstream_method: Some(upstream_request.method.clone())
    })
}

async fn handle_request(request: APIRequest, data: &web::Data<AppData>, client_ip: &String) -> Result<APICallResponse, ErrorStructure> {
    // perform any requested mappings, this may give us different method names & and params
    let mapped_method = match method_renamer::map_method_name(&data.config, &request.method, &request.params) {
        Ok(mapped_method) => { mapped_method }
        Err(_) => {
            return Err(ErrorStructure {
                jsonrpc: request.jsonrpc.clone(),
                id : request.id.clone(),
                code: -32700,
                message: format!("Unable to parse request method."),
                error: ErrorField::Message("Unable to parse request method.".to_string()),
            });
        }
    };

    check_for_future_block_requests(&mapped_method);

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

    // Check if the call is in the cache. If it is, return it
    let params_str = request.params.as_ref().map_or("[]".to_string(), |v: &Value| v.to_string());
    let method_and_params_str = format!("{}({params_str})", mapped_method.method);
    println!("Using cache key {method_and_params_str}");
    if let Some(cached_call) = data.cache.get(&method_and_params_str) {
        println!("Cache hit");
        return Ok(APICallResponse {
            jsonrpc: request.jsonrpc.clone(),
            result: cached_call.result.clone(),
            id: request.id,
            cached: true,
            mapped_method,
            backend_url: None,
            upstream_method: None
        });
    }
    println!("Cache miss");

    // Check and see if another thread is currently handling this call.  If it is, join them
    let in_progress_call: InProgressCall;
	let first_caller: bool;
    {
        let mut in_progress = in_progress_call_registry().lock().unwrap();
        in_progress_call = if let Some(value) = in_progress.get_mut(&method_and_params_str) {
			first_caller = false;
            value.call_count += 1;
            value.clone()
        } else {
			first_caller = true;
            let upstream_request_record = InProgressCall {
                future: request_from_upstream(request, data.clone(), mapped_method, method_and_params_str.clone()).boxed().shared(),
                call_count: 1,
                first_call_start_time: Instant::now()
            };
            in_progress.insert(method_and_params_str.clone(), upstream_request_record.clone());
            upstream_request_record
        }
    }
    if !first_caller {
        println!("Multiple calls in progress: {method_and_params_str}: {}", in_progress_call.call_count);
    }

    let upstream_response = in_progress_call.future.await;

    // one caller needs to remove the entry from the registry when the upstream call has completed.
    // It doesn't matter who does it, so we arbitrarily decide to make it the first caller.
	if first_caller	{
        in_progress_call_registry().lock().unwrap().remove(&method_and_params_str);
    }

    upstream_response
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
            let result = handle_request(request, &data, &user_ip).await;
            match result {
                Ok(response) => HttpResponse::Ok()
                    .insert_header(("Drone-Version", DRONE_VERSION))
                    .insert_header(("X-Jussi-Cache-Hit", response.cached.to_string()))
                    .insert_header(("X-Jussi-Namespace", response.mapped_method.namespace))
                    .insert_header(("X-Jussi-Api", response.mapped_method.api.unwrap_or("<Empty>".to_string())))
                    .insert_header(("X-Jussi-Method", response.mapped_method.method))
                    .insert_header(("X-Jussi-Params", response.mapped_method.params.map_or("[]".to_string(), |v| v.to_string())))
                    .insert_header(("X-Jussi-Backend-Url", response.backend_url.unwrap_or("".to_string())))
                    .insert_header(("X-Jussi-Upstream-Method", response.upstream_method.unwrap_or("".to_string())))
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
                let result = handle_request(request, &data, &user_ip).await;
                match result {
                    Ok(response) => responses.push(response),
                    Err(err) => return HttpResponse::InternalServerError().json(err),
                }
            }
            let mut cached = true;
            let mut result = Vec::new();
            for response in responses {
                if !response.cached {
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

#[derive(Clone)]
struct InProgressCall {
    future: Shared<Pin<Box<dyn Future<Output = Result<APICallResponse, ErrorStructure>> + Send>>>,
    /// the rest of this data structure is just info we track for debugging/logging,
    /// we could omit them and Drone would work the same
    call_count: u32,
    first_call_start_time: Instant,
}

struct AppData {
    cache: Cache<String, CacheEntry>,
    webclient: Client,
    config: AppConfig,
}

/// Singleton registry that holds futures for all currently-in-progress calls to the upstream servers,
/// indexed by method name and parameters
fn in_progress_call_registry() -> &'static Mutex<HashMap<String, InProgressCall>> {
    static INSTANCE: OnceLock<Mutex<HashMap<String, InProgressCall>>> = OnceLock::new();
    INSTANCE.get_or_init(|| Mutex::new(HashMap::new()))
}

#[actix_web::main]
async fn main() -> std::io::Result<()> {
    // Load config.
    let app_config = config::parse_file("config.yaml");

    // helpers for the cach
    let expiry = MyExpiry;
    let eviction_listener = |key, _value, cause| {
        println!("Evicted key {key}. Cause: {cause:?}");
    };
    let weigher = |_key: &String, value: &CacheEntry| -> u32 {
        value.size
    };

    // Create the cache.
    let _cache = web::Data::new(AppData {
        cache: Cache::builder()
            .max_capacity(app_config.drone.cache_max_capacity)
            .expire_after(expiry)
            .eviction_listener(eviction_listener)
            .weigher(weigher)
            .build(),
        webclient: ClientBuilder::new()
            .pool_max_idle_per_host(app_config.drone.middleware_connection_threads)
            .build()
            .unwrap(),
        config: app_config.clone(),
    });
    println!("Drone is running on port {}.", app_config.drone.port);
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
    .bind((app_config.drone.hostname, app_config.drone.port))?
    .run()
    .await
}
