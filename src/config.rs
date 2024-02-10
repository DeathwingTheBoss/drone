use std::collections::{HashMap, HashSet};
use sequence_trie::SequenceTrie;
use serde::{Deserialize};
use std::fs;


#[derive(Debug, Clone, Copy, PartialEq)]
pub enum TtlValue {
    /// Never cache the response
    NoCache,
    /// Cache the response forever
    NoExpire,
    /// If irreversible, cache forever.  Otherwise, cache for 9 seconds
    ExpireIfReversible,
    /// Expect the upstream server to tell us how long to cache via a `Cache-Control: max-age=`  HTTP header
    HonorUpstreamCacheControl,
    /// Cache response for a fixed amount of time
    DurationInSeconds(u32)
}

#[derive(Clone, Deserialize, Debug)]
#[serde()]
pub struct DroneConfig {
    /// the port to listen on
    pub port: u16,
    /// the endpoint (usually an IP like 0.0.0.0, but can be a hostname if you're weird)
    pub hostname: String,

    //cache_initial_capacity: Option<u64>, // <- not sure we have any use for this
    /// initial cache capacity, roughly the max size in bytes
    pub cache_max_capacity: u64,

    /// String used to identify this software, returned in the response for / and /health endpoints
    pub operator_message: String,

    /// number of threads used for connections between drone and the backends
    pub middleware_connection_threads: usize,
}


/// AppConfig holds the parsed, ready-to-use configuration
#[derive(Debug, Clone)]
pub struct AppConfig {
    pub drone: DroneConfig,
    backends: HashMap<String, String>,
    pub translate_to_appbase: HashSet<String>,
    pub urls: SequenceTrie<String, String>,
    pub ttls: SequenceTrie<String, TtlValue>,
    pub timeouts: SequenceTrie<String, u32>,
    pub equivalent_methods: HashMap<String, String>,
}

impl AppConfig {
    pub fn lookup_url(&self, method_name_parts: Vec<&String>) -> Option<&String> {
        self.urls.get_ancestor(method_name_parts)
    }
    pub fn lookup_ttl(&self, method_name_parts: Vec<&String>) -> Option<&TtlValue> {
        self.ttls.get_ancestor(method_name_parts)
    }
    pub fn lookup_timeout(&self, method_name_parts: Vec<&String>) -> Option<&u32> {
        self.timeouts.get_ancestor(method_name_parts)
    }
    pub fn lookup_equivalent_method(&self, original_method_name: String) -> String {
        match self.equivalent_methods.get(&original_method_name) {
            Some(replacement_method_name) => { replacement_method_name.clone() }
            None => { original_method_name }
        }
    }
}

/// TtlValue enum split into two to make serde read it correctly
#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
enum YamlSpecialTtlValue {
    NoCache,
    NoExpire,
    ExpireIfReversible,
    HonorUpstreamCacheControl,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(untagged)]
enum YamlTtlValue {
    SpecialValue(YamlSpecialTtlValue),
    DurationInSeconds(u32)
}

/// and a helper function to hide the ugliness
fn ttl_value_from_yaml(ttl: &YamlTtlValue) -> TtlValue {
    match ttl {
        YamlTtlValue::SpecialValue(YamlSpecialTtlValue::NoCache) => { TtlValue::NoCache }
        YamlTtlValue::SpecialValue(YamlSpecialTtlValue::NoExpire) => { TtlValue::NoExpire }
        YamlTtlValue::SpecialValue(YamlSpecialTtlValue::ExpireIfReversible) => { TtlValue::ExpireIfReversible }
        YamlTtlValue::SpecialValue(YamlSpecialTtlValue::HonorUpstreamCacheControl) => { TtlValue::HonorUpstreamCacheControl }
        YamlTtlValue::DurationInSeconds(n) => { TtlValue::DurationInSeconds(*n) } 
    }
}

/// The config file format directly loaded from file
#[derive(Debug, Deserialize)]
struct YamlConfig {
    drone: DroneConfig,
    backends: HashMap<String, String>,
    translate_to_appbase: Vec<String>,
    urls: HashMap<String, String>,
    ttls: HashMap<String, YamlTtlValue>,
    timeouts: HashMap<String, u32>,
    equivalent_methods: HashMap<String, Vec<String>>,
}

pub fn parse_file(filename: &str) -> AppConfig {
    // Parse the YAML into a temporary structure
    let yaml_string = fs::read_to_string(filename)
        .expect("Unable to read file");

    let config: YamlConfig = serde_yaml::from_str(&yaml_string)
        .expect("Unable to parse YAML");

    // Then move the data into our AppConfig, into a format that's easier to use at runtime
    let mut app_config = AppConfig {
        drone: DroneConfig{port: 80, hostname: "0.0.0.0".to_string(), cache_max_capacity: 4 << 30, operator_message: "Drone by Deathwing".to_string(), middleware_connection_threads: 8},
        backends: HashMap::new(),
        translate_to_appbase: HashSet::new(),
        urls: SequenceTrie::new(),
        ttls: SequenceTrie::new(),
        timeouts: SequenceTrie::new(),
        equivalent_methods: HashMap::new()
    };

    app_config.drone = config.drone;
    // copy the backends in to our AppConfig.  We don't have any immediate use for them now
    for (backend, url) in config.backends {
        app_config.backends.insert(backend, url);
    }
    for namespace in config.translate_to_appbase {
        app_config.translate_to_appbase.insert(namespace);
    }

    // move the urls, resolving the backend names into urls as we go
    for (method, backend_name) in config.urls {
        let parts: Vec<String> = method.split('.').map(|v| v.to_string()).collect();
        let url = app_config.backends.get(&backend_name).unwrap_or_else(|| panic!("Undefined backend \"{}\" for method \"{}\"", backend_name, method));
        app_config.urls.insert_owned(parts, url.clone());
    }
    for (method, ttl) in config.ttls {
        let parts: Vec<String> = method.split('.').map(|v| v.to_string()).collect();
        app_config.ttls.insert_owned(parts, ttl_value_from_yaml(&ttl));
    }
    for (method, timeout) in config.timeouts {
        let parts: Vec<String> = method.split('.').map(|v| v.to_string()).collect();
        app_config.timeouts.insert_owned(parts, timeout);
    }
    // flip the equivalent_methods around so we can do a direct source -> target lookup
    for (target_method, source_methods) in config.equivalent_methods {
        for source_method in source_methods {
            app_config.equivalent_methods.insert(source_method, target_method.clone());
        }
    }
    app_config
}
