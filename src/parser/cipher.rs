use boa_engine::Context;
use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct DecipherQuery {
    url: String,
    s: String,
    sp: Option<String>,
}

#[cfg_attr(feature = "performance_analysis", flamer::flame)]
fn decode_url(url: &str) -> Option<DecipherQuery> {
    serde_qs::from_str(url).ok()
}

#[cfg_attr(feature = "performance_analysis", flamer::flame)]
pub fn decipher(
    base_url: &str,
    decipher_script_string: &(String, String),
    cipher_cache: &mut Option<(String, Context)>,
) -> String {
    if decipher_script_string.1.is_empty() {
        return base_url.to_string();
    }
    let args: DecipherQuery = if let Some(e) = decode_url(base_url) {
        e
    } else {
        return base_url.to_string();
    };

    let url = args.url;
    let s = args.s;

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
                return url;
            }
            *cipher_cache = Some((decipher_script_string.1.to_string(), context));
            &mut cipher_cache.as_mut().unwrap().1
        }
    };

    let result = {
        #[cfg(feature = "performance_analysis")]
        let _guard = flame::start_guard("execute engine");
        context
            .eval(boa_engine::Source::from_bytes(&format!(
                r#"{func_name}("{args}")"#,
                func_name = decipher_script_string.0.as_str(),
                args = s
            )))
            .expect("Can't execute script")
    };

    let convert_result_to_rust_string = result
        .as_string()
        .expect("Can't convert to string")
        .to_std_string()
        .expect("Can't convert to string");

    let mut return_url = url::Url::parse(&url).expect("Can't parse the url");

    let query_name = args.sp.unwrap_or_else(|| "signature".to_owned());

    let mut query = return_url
        .query_pairs()
        .map(|(name, value)| {
            if name == query_name {
                (name.into_owned(), convert_result_to_rust_string.to_string())
            } else {
                (name.into_owned(), value.into_owned())
            }
        })
        .collect::<Vec<(String, String)>>();

    if !return_url.query_pairs().any(|(name, _)| name == query_name) {
        query.push((query_name.to_string(), convert_result_to_rust_string));
    }

    return_url.query_pairs_mut().clear().extend_pairs(&query);

    return_url.to_string()
}
