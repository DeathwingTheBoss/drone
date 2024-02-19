use actix_cors::Cors;
use actix_web::{web, App, HttpRequest, HttpResponse, HttpServer, Responder};
use moka::{future::Cache, Expiry};
use reqwest::{Client, ClientBuilder};
use serde::{Deserialize, Serialize, Serializer};
use serde_json::Value;
use std::sync::{Arc};
use std::time::{Duration, Instant, SystemTime};
use actix_web::rt::time::sleep;
use tokio::sync::RwLock;
use chrono::DateTime;



pub mod config;
use config::{AppConfig, TtlValue};

pub mod method_renamer;
use method_renamer::MethodAndParams;

const DRONE_VERSION: &str = env!("CARGO_PKG_VERSION");

struct BlockchainState {
    last_irreversible_block_number: u32,
    head_block_number: u32,
    head_block_time: SystemTime
}

impl BlockchainState {
    pub fn new() -> BlockchainState {
        BlockchainState {
            last_irreversible_block_number: 0,
            head_block_number: 0,
            head_block_time: SystemTime::UNIX_EPOCH
        }
    }
}


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

#[derive(Clone, Debug)]
struct ErrorData {
    code: i32,
    message: String,
    error: ErrorField,
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

#[derive(Clone, Debug)]
struct ApiCallResponseData {
    result: Value,
    // the following fields are for emitting debugging headers, not strictly necessary:
    backend_url: String,
    upstream_method: String
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
    backend_url: String,
    upstream_method: String
}

#[derive(Debug, Copy, Clone)]
enum CacheTtl {
    NoCache,
    NoExpire,
    CacheForDuration(Duration)
}

#[derive(Clone, Debug)]
struct CacheEntry {
    result: Result<ApiCallResponseData, ErrorData>,
    size: u32,
	ttl: CacheTtl,
}

pub struct MyExpiry;

impl MyExpiry {
    fn get_expiration(&self, key: &String, value: &CacheEntry) -> Option<Duration> {
		match value.ttl {
            CacheTtl::NoExpire => { 
                println!("MyExpiry: get_expiration called with key {key}, returning duration None (never expire).");
                None
            }
            CacheTtl::NoCache => {
                println!("MyExpiry: get_expiration called with key {key}, returning duration 0 (don't cache).");
                Some(Duration::ZERO)
            }
            CacheTtl::CacheForDuration(duration) => {
                println!("MyExpiry: get_expiration called with key {key}, returning duration {:?}", duration);
                Some(duration)
            }
        }
    }
}

impl Expiry<String, CacheEntry> for MyExpiry {
    /// Returns the duration of the expiration of the value that was just
    /// created.
    fn expire_after_create(&self, key: &String, value: &CacheEntry, _current_time: Instant) -> Option<Duration> {
        self.get_expiration(key, value)
    }
    /// We never explicitly update cache entries, we keep serving data from the cache until the
    /// cache entry expires, then when we get a cache miss we make another call to the upstream and
    /// insert the new value.
    /// But it appears that there's some lazyness -- after an entry's expiration time has passed,
    /// get() calls will return None, but the entry will still exist in the cache for a while until
    /// the entry is actually evicted, maybe on the order of ~0.3s.  If we insert a new value
    /// during that window, I think it considers that an "update" and not a "create", so we need to
    /// override expire_after_update too.
    fn expire_after_update(&self, key: &String, value: &CacheEntry, 
                           _updated_at: Instant,
                           _duration_until_expiry: Option<Duration>) -> Option<Duration> {
        self.get_expiration(key, value)
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
// - returning a stock error reply without contacting the upstream, or
// - if the block is expected to arrive in a few seconds, just wait.  once the block arrives, return it
// Waiting seems better, because if we don't, the client will probably just make the same request
// again (maybe after a short sleep).  And if we do it right, it may give them the block sooner
// than their polling loop would have.
async fn check_for_future_block_requests(mapped_method: &MethodAndParams, data: &web::Data<AppData>) {
    if mapped_method.method == "get_block" {
        if let Some(block_num) = mapped_method.params.as_ref().and_then(|v| v["block_num"].as_u64()) {
            let current_head_block_number = data.blockchain_state.read().await.head_block_number;
            if block_num as u32 > current_head_block_number {
                // we're only testing against the head block number we recorded the last
                // time someone called get_dynamic_global_properties.
                // we should also check that now() is < the predicted time the requested
                // block will be produced
                println!("future block requested: {block_num}, head is {current_head_block_number}");
            }
        }
    }
}

async fn request_from_upstream(data: web::Data<AppData>, mapped_method: MethodAndParams, method_and_params_str: String) -> CacheEntry {
    let endpoint = match data.config.lookup_url(mapped_method.get_method_name_parts()) {
        Some(endpoint) => { endpoint }
        None => {
            return CacheEntry {
                result: Err(ErrorData {
                    code: -32603, // or 32601?
                    message: "Unable to map request to endpoint.".to_string(),
                    error: ErrorField::Message("Unable to map request to endpoint.".to_string()),
                }),
                size: 0,
                ttl: CacheTtl::NoCache
            };
        }
    };

    let upstream_request = mapped_method.format_for_upstream(&data.config);
    println!("Making upstream request for {method_and_params_str}");
    // using method {:?} and params {:?}", upstream_request.method, upstream_request.params);

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
            return CacheEntry {
                result: Err(ErrorData {
                    code: -32700,
                    message: "Unable to send request to endpoint.".to_string(),
                    error: ErrorField::Message(error_message),
                }),
                size: 0,
                ttl: CacheTtl::NoCache
            };
        }
    };

    // to simulate slow calls, put a sleep here
    sleep(Duration::from_secs(10)).await;

    let body = match res.text().await {
        Ok(text) => text,
        Err(err) => {
            return CacheEntry {
                result: Err(ErrorData {
                    code: -32600,
                    message: "Received an invalid response from the endpoint.".to_string(),
                    error: ErrorField::Message(err.to_string()),
                }),
                size: 0,
                ttl: CacheTtl::NoCache
            };
        }
    };
    let mut json_body: serde_json::Value = match serde_json::from_str(&body) {
        Ok(parsed) => parsed,
        Err(err) => {
            return CacheEntry {
                result: Err(ErrorData {
                    code: -32602,
                    message: "Unable to parse endpoint data.".to_string(),
                    error: ErrorField::Message(err.to_string()),
                }),
                size: 0,
                ttl: CacheTtl::NoCache
            };
        }
    };
    if json_body["error"].is_object() {
        return CacheEntry {
            result: Err(ErrorData {
                code: -32700,
                message: "Endpoint returned an error.".to_string(),
                error: ErrorField::Object(json_body["error"].clone()),
            }),
            size: 0,
            ttl: CacheTtl::NoCache
        };
    }

    // if the call was to get_dynamic_global_properties, save off the last irreversible block
    println!("Mapped method is {}", mapped_method.method);
    if mapped_method.method == "get_dynamic_global_properties" {
        println!("This method was get_dynamic_global_properties");

        let new_lib = json_body["result"]["last_irreversible_block_num"].as_u64().map(|v| v as u32);
        let new_head = json_body["result"]["head_block_number"].as_u64().map(|v| v as u32);
        let new_time = json_body["result"]["time"].as_str();
        match (new_lib, new_head, new_time) {
            (Some(new_lib), Some(new_head), Some(new_time)) => {
                let read_lock = data.blockchain_state.read().await;
                if new_lib > read_lock.last_irreversible_block_number || new_head > read_lock.last_irreversible_block_number {
                    drop(read_lock);
                    let mut write_lock = data.blockchain_state.write().await;
                    write_lock.last_irreversible_block_number = new_lib;
                    if new_head != write_lock.head_block_number {
                        write_lock.head_block_number = new_head;
                        let current_head_block_time = DateTime::parse_from_rfc3339(&format!("{new_time}Z")).unwrap();
                        write_lock.head_block_time = SystemTime::from(current_head_block_time);
                    }
                }
            }
            _ => {
                println!("Invalid get_dynamic_global_properties result, ignoring");
            }
        }
    }

    let ttl = if json_body["result"].is_array() && json_body["result"].as_array().unwrap().is_empty()
                 || json_body["result"].is_null()
                 || json_body["result"]["blocks"].is_array() && json_body["result"]["blocks"].as_array().unwrap().is_empty()
    {
        // then this result shouldn't be cached
        // TODO: why?
        CacheTtl::NoCache
    }
    else
    {
        let ttl_from_config = *data.config.lookup_ttl(mapped_method.get_method_name_parts()).unwrap_or(&TtlValue::NoCache);
        println!("lookup_ttl for {method_and_params_str} returns {ttl_from_config:?}");

        match ttl_from_config {
            TtlValue::NoCache => { CacheTtl::NoCache }
            TtlValue::NoExpire => { CacheTtl::NoExpire }
            TtlValue::ExpireIfReversible => {
                // we cache forever if the block is irreversible, or 9 seconds if it's reversible
                if let Some(block_number) = get_block_number_from_result(&json_body["result"]) {
                    let last_irreversible_block_number = data.blockchain_state.read().await.last_irreversible_block_number;
                    if block_number > last_irreversible_block_number { CacheTtl::CacheForDuration(Duration::from_secs(9)) } else { CacheTtl::NoExpire }
                } else {
                    // we couldn't extract a block number from the result.  probably an error
                    // result, or the config has specified ExpireIfReversible for a call that isn't
                    // supported by get_block_number_from_result
                    CacheTtl::NoCache
                }
            }
            TtlValue::HonorUpstreamCacheControl => { CacheTtl::NoCache /* TODO: implement this */ }
            TtlValue::DurationInSeconds(seconds) => { CacheTtl::CacheForDuration(Duration::from_secs(seconds as u64)) }
        }
    };

    CacheEntry {
        result: Ok(ApiCallResponseData {
            result: json_body["result"].take(),
            backend_url: endpoint.to_string(),
            upstream_method: upstream_request.method.to_string()
        }),
        size: body.len() as u32,
        ttl
    }
}

async fn handle_request(request: APIRequest, data: &web::Data<AppData>, client_ip: &String) -> Result<APICallResponse, ErrorStructure> {
    // perform any requested mappings, this may give us different method names & and params
    let mapped_method = method_renamer::map_method_name(&data.config, &request.method, &request.params).or_else(|_| 
        Err(ErrorStructure {
            jsonrpc: request.jsonrpc.clone(),
            id : request.id.clone(),
            code: -32700,
            message: "Unable to parse request method.".to_string(),
            error: ErrorField::Message("Unable to parse request method.".to_string()),
        })
    )?;

    check_for_future_block_requests(&mapped_method, data).await;

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

    // Get the result of the call.  The get_with() call below will:
    // - see if the result of the call is cached.  If so, return the result immediately
    // - if not, it checks whether some other task is currently calling the upstream to
    //   get the result of this same call.  If so, it will just share their result instead
    //   of initiating a second call
    // - otherwise, it will call the closure (request_from_upstream()) to get the result,
    //   then insert it into the cache
    //
    // notes: 
    // - moka is in charge of this behavior, and it behaves the way we want, but that means
    //   we have less information about exactly what happened when we call get_with().  
    //   We say that the result is "cached" if the closure didn't get executed; that could
    //   mean that the result was already in the cache, or it could mean that another task/thread
    //   was already in the process of requesting it.  That means times for some "cached" calls
    //   could be as long as non-cached calls.  Just something to be aware of.
    // - to get this "combining multiple simultaneous calls" behavior, we're inserting every
    //   result into the cache, even if it's marked as something we don't want to cache in the
    //   config (we just insert them with a TTL of zero).  That may cause some 
    //   unnecessary/unwanted effects, but so far performance seems to be the same compared to
    //   an alternate implementation where the caching and combining were handled separately.
    let params_str = request.params.as_ref().map_or("[]".to_string(), |v: &Value| v.to_string());
    let method_and_params_str = mapped_method.get_dotted_method_name() + "(" + &params_str + ")";

    let mut upstream_was_called = false;
    let cache_entry = data.cache.get_with_by_ref(&method_and_params_str,
                                                 async { upstream_was_called = true; request_from_upstream(data.clone(), mapped_method.clone(), method_and_params_str.clone()).await }).await;

    match cache_entry.result {
        Ok(api_call_response) => {
            Ok(APICallResponse {
                jsonrpc: request.jsonrpc.clone(),
                result: api_call_response.result.clone(),
                id: request.id,
                cached: !upstream_was_called,
                mapped_method,
                backend_url: api_call_response.backend_url.to_string(),
                upstream_method: api_call_response.upstream_method.to_string()
            })
        }
        Err(error_data) => {
            Err(ErrorStructure {
                jsonrpc: request.jsonrpc.clone(),
                id : request.id,
                code: error_data.code,
                message: error_data.message.to_string(),
                error: error_data.error.clone()
            })
        }
    }
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
                    .insert_header(("X-Jussi-Backend-Url", response.backend_url))
                    .insert_header(("X-Jussi-Upstream-Method", response.upstream_method))
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

struct AppData {
    cache: Cache<String, CacheEntry>,
    webclient: Client,
    config: AppConfig,
    blockchain_state: Arc<RwLock<BlockchainState>>
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
        blockchain_state: Arc::new(RwLock::new(BlockchainState::new()))
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
