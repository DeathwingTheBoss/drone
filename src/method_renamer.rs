use serde_json::{Value};
use serde_json::json;


use super::config::AppConfig;
use super::APIRequest;
use super::ID;

#[derive(Clone, Debug)]
pub struct MethodAndParams {
    pub namespace: String,
    pub api: Option<String>,
    pub method: String,
    pub params: Option<Value>,
    pub full_method: String // the method as called (usually api.method)
}

impl MethodAndParams {
    /// get [namespace, api, method], or just [namespace, method] if no API
    // this is the format used to lookup methods in tries
    pub fn get_method_name_parts(&self) -> Vec<&String> {
        match &self.api {
            Some(api) => { vec![&self.namespace, &api, &self.method] }
            None => { vec![&self.namespace, &self.method] }
        } 
    }

    /// same as get_method_name_parts, but as a dotted string
    // this is the format our equivalent_method map uses
    pub fn get_dotted_method_name(&self) -> String {
        match &self.api {
            Some(api) => { [self.namespace.as_str(), api.as_str(), self.method.as_str()].join(".") }
            None => { [self.namespace.as_str(), self.method.as_str()].join(".") }
        } 
    }

    /// gets the request to pass to the upstream
    pub fn format_for_upstream(&self, _app_config: &AppConfig) -> APIRequest {
        // app_config isn't currently used, but we could use it if we want to, say,
        // send all translate_to_appbase calls using the "call" syntax
        APIRequest {
            id: ID::Int(1),
            jsonrpc: "2.0".to_string(),
            method: self.full_method.to_string(),
            params: self.params.clone()
        }
    }
}

fn decode_api_if_numeric(api: &Value) -> Result<String, String> {
    if let Some(number) = api.as_i64() {
        match number {
            0 => { Ok("database_api".to_string()) }
            1 => { Ok("login_api".to_string()) }
            _ => { Err("Invalid API number".to_string()) }
        }
    } else {
        match api.as_str() {
            Some(api_string) => { Ok(api_string.to_string()) }
            None => { Err("Invalid API".to_string()) }
        }
    }
}

fn parse_call(params: &Option<Value>) -> Result<MethodAndParams, String> {
    match params.as_ref().and_then(Value::as_array).map(Vec::as_slice) {
        Some([api, method]) => {
            let api_part = decode_api_if_numeric(api)?;
            let method_part = method.as_str().ok_or_else(|| "Invalid method name".to_string())?;
            let full_method = api_part.to_string() + "." + method_part;

            Ok(MethodAndParams {
                namespace: "appbase".to_string(),
                api: Some(api_part),
                method: method_part.to_string(),
                params: None,
                full_method
            })
        }
        Some([api, method, actual_params]) => {
            let api_part = decode_api_if_numeric(api)?;
            let method_part = method.as_str().ok_or_else(|| "Invalid method name".to_string())?;
            let full_method = api_part.to_string() + "." + method_part;

            Ok(MethodAndParams {
                namespace: if api == "condenser_api" || api == "jsonrpc" || actual_params.is_object() { "appbase".to_string() } else { "hived".to_string() },
                api: Some(api_part),
                method: method_part.to_string(),
                params: Some(actual_params.clone()),
                full_method
            })
        }
        _ => { 
            Err("error".to_string())
        }
    }
}

fn convert_method_name(full_method: &str, params: &Option<Value>) -> Result<MethodAndParams, String> {
    let parts: Vec<&str> = full_method.split('.').collect();
    match &parts[..] {
        ["call"] => {
            parse_call(params)
        }
        [bare_method] => {
            Ok(MethodAndParams {
                namespace: "hived".to_string(),
                api: Some("database_api".to_string()), 
                method: bare_method.to_string(),
                params: params.clone(),
                full_method: full_method.to_string()
            })
        }
        [appbase_api, method] if appbase_api.ends_with("_api") => {
            // e.g., condenser_api.method
            Ok(MethodAndParams {
                namespace: "appbase".to_string(),
                api: Some(appbase_api.to_string()),
                method: method.to_string(),
                params: params.clone(),
                full_method: full_method.to_string()
            })
        }
        ["jsonrpc", method] => {
            Ok(MethodAndParams {
                namespace: "appbase".to_string(),
                api: Some("jsonrpc".to_string()),
                method: method.to_string(),
                params: params.clone(),
                full_method: full_method.to_string()
            })
        }
        [namespace, method] => {
            // in practice, this is usually bridge.method
            // {namespace, None, method}
            Ok(MethodAndParams {
                namespace: namespace.to_string(),
                api: None,
                method: method.to_string(),
                params: params.clone(),
                full_method: full_method.to_string()
            })
        }
        [namespace, api, method] => {
            Ok(MethodAndParams {
                namespace: namespace.to_string(),
                api: Some(api.to_string()),
                method: method.to_string(),
                params: params.clone(),
                full_method: full_method.to_string()
            })
        }
        _ => {
            Err("Error parsing method".to_string())
        }
    }
}

fn translate_to_appbase(method_and_params: MethodAndParams) -> Result<MethodAndParams, String> {
	parse_call(&Some(json!(["condenser_api", method_and_params.method, &method_and_params.params.or(Some(json!([])))])))
}

/// the main method in this module, this:
/// - parses the request, figuring out what "namespace", "api", and "method" the call represents
///   using the same rules jussi uses
/// - applies part of the "translate_to_appbase" functionality from jussi: if the call is marked
///   as one that should be translated, it converts it to the "appbase" namespace and
///   "condenser_api".  However, unlike jussi, it does not cause the call to the upstream to be 
///   made using the "call" syntax
/// - does any method renaming requested by the "equivalent_methods" config
pub fn map_method_name(app_config: &AppConfig, method: &str, params: &Option<Value>) -> Result<MethodAndParams, String> {
	let mut mapped_method_and_params = convert_method_name(method, params)?;

	if app_config.translate_to_appbase.contains(&mapped_method_and_params.namespace) {
		mapped_method_and_params = translate_to_appbase(mapped_method_and_params)?;
	}

	match app_config.equivalent_methods.get(&mapped_method_and_params.get_dotted_method_name()) {
        // note: the user can cause infinite recursion by setting up an equivalent_methods rule
        // that renames to itself
		Some(new_method_name) => { map_method_name(app_config, new_method_name, &mapped_method_and_params.params) }
		None => { Ok(mapped_method_and_params) }
	}
}
