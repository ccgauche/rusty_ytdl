use boa_engine::Context;

#[cfg_attr(feature = "performance_analysis", flamer::flame)]
pub fn decipher(
    url: &str,
    decipher_script_string: &(String, String),
    cipher_cache: &mut Option<(String, Context)>,
) -> String {
    let args: serde_json::value::Map<String, serde_json::Value> = {
        #[cfg(feature = "performance_analysis")]
        let _guard = flame::start_guard("serde_qs::from_str");
        serde_qs::from_str(url).unwrap()
    };

    if args.get("s").is_none() || decipher_script_string.1.is_empty() {
        if args.get("url").is_none() {
            return url.to_string();
        } else {
            let args_url = args.get("url").and_then(|x| x.as_str()).unwrap_or("");
            return args_url.to_string();
        }
    }

    let context = match cipher_cache {
        Some((ref cache_key, ref mut context)) if cache_key == &decipher_script_string.1 => context,
        _ => {
            #[cfg(feature = "performance_analysis")]
            let _guard = flame::start_guard("build engine");
            let mut context = boa_engine::Context::default();
            let decipher_script = context.eval(boa_engine::Source::from_bytes(
                decipher_script_string.1.as_str(),
            ));
            if decipher_script.is_err() {
                if args.get("url").is_none() {
                    return url.to_string();
                } else {
                    let args_url = args.get("url").and_then(|x| x.as_str()).unwrap_or("");
                    return args_url.to_string();
                }
            }
            *cipher_cache = Some((decipher_script_string.1.to_string(), context));
            &mut cipher_cache.as_mut().unwrap().1
        }
    };

    let result = {
        #[cfg(feature = "performance_analysis")]
        let _guard = flame::start_guard("execute engine");
        context.eval(boa_engine::Source::from_bytes(&format!(
            r#"{func_name}("{args}")"#,
            func_name = decipher_script_string.0.as_str(),
            args = args.get("s").and_then(|x| x.as_str()).unwrap_or("")
        )))
    };

    if result.is_err() {
        if args.get("url").is_none() {
            return url.to_string();
        } else {
            let args_url = args.get("url").and_then(|x| x.as_str()).unwrap_or("");
            return args_url.to_string();
        }
    }

    let is_result_string = result.as_ref().unwrap().as_string();

    if is_result_string.is_none() {
        if args.get("url").is_none() {
            return url.to_string();
        } else {
            let args_url = args.get("url").and_then(|x| x.as_str()).unwrap_or("");
            return args_url.to_string();
        }
    }

    let convert_result_to_rust_string = is_result_string.unwrap().to_std_string();

    if convert_result_to_rust_string.is_err() {
        if args.get("url").is_none() {
            return url.to_string();
        } else {
            let args_url = args.get("url").and_then(|x| x.as_str()).unwrap_or("");
            return args_url.to_string();
        }
    }

    let result = convert_result_to_rust_string.unwrap();

    let return_url = url::Url::parse(args.get("url").and_then(|x| x.as_str()).unwrap_or(""));

    if return_url.is_err() {
        if args.get("url").is_none() {
            return url.to_string();
        } else {
            let args_url = args.get("url").and_then(|x| x.as_str()).unwrap_or("");
            return args_url.to_string();
        }
    }

    let mut return_url = return_url.unwrap();

    let query_name = if args.get("sp").is_some() {
        args.get("sp")
            .and_then(|x| x.as_str())
            .unwrap_or("signature")
    } else {
        "signature"
    };

    let mut query = return_url
        .query_pairs()
        .map(|(name, value)| {
            if name == query_name {
                (name.into_owned(), result.to_string())
            } else {
                (name.into_owned(), value.into_owned())
            }
        })
        .collect::<Vec<(String, String)>>();

    if !return_url.query_pairs().any(|(name, _)| name == query_name) {
        query.push((query_name.to_string(), result));
    }

    return_url.query_pairs_mut().clear().extend_pairs(&query);

    return_url.to_string()
}
