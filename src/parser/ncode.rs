use std::collections::HashMap;

use boa_engine::{Context, JsValue, Source};
use url::Url;
use urlencoding::decode;

#[cfg_attr(feature = "performance_analysis", flamer::flame)]
// Caching this would be great (~2ms x 2 gain/req on Ryzen 9 5950XT) but is quite hard because of the !Send nature of boa
fn create_transform_script(script: &str) -> Context<'_> {
    let mut context = boa_engine::Context::default();
    context.eval(parse_source(script)).unwrap();
    context
}

#[cfg_attr(feature = "performance_analysis", flamer::flame)]
fn parse_source(script: &str) -> Source<&[u8]> {
    boa_engine::Source::from_bytes(script)
}

#[cfg_attr(feature = "performance_analysis", flamer::flame)]
// Optimizing the script would be great (~20ms x 2 gain/req on Ryzen 9 5950XT) but quite some work on boa side
// This is where most of the time is spent
fn execute_transform_script(
    context: &mut Context,
    func_name: &str,
    n_transform_value: &str,
) -> JsValue {
    context
        .eval(parse_source(&format!(
            r#"{func_name}("{n_transform_value}")"#,
        )))
        .unwrap()
}

#[cfg_attr(feature = "performance_analysis", flamer::flame)]
pub fn ncode(
    url: &str,
    n_transform_script_string: &(String, String),
    n_transfrom_cache: &mut HashMap<String, String>,
) -> String {
    if n_transform_script_string.1.is_empty() {
        return url.to_string();
    }
    let mut return_url = Url::parse(url).expect("Can't parse the url");
    let n = return_url.query_pairs().find(|(name, _)| name == "n").map(|(_,v)| v);

    let n = if let Some(n) = &n {
        n.as_ref()
    } else {
        return url.to_string();
    };

    let n_transform_result = if let Some(&result) = n_transfrom_cache.get(n).as_ref() {
        result.clone()
    } else {
        let mut context = create_transform_script(n_transform_script_string.1.as_str());

        let is_result_string = execute_transform_script(
            &mut context,
            n_transform_script_string.0.as_str(),
            &n,
        );

        let is_result_string = is_result_string.as_string();

        if is_result_string.is_none() {
            return url.to_string();
        }

        let convert_result_to_rust_string = is_result_string.unwrap().to_std_string();

        if convert_result_to_rust_string.is_err() {
            return url.to_string();
        }

        let result = convert_result_to_rust_string.unwrap();

        n_transfrom_cache.insert(n.to_owned(), result.clone());

        result
    };

    let query = return_url
        .query_pairs()
        .map(|(name, value)| {
            if name == "n" {
                (name.into_owned(), n_transform_result.to_string())
            } else {
                (name.into_owned(), value.into_owned())
            }
        })
        .collect::<Vec<(String, String)>>();

    return_url.query_pairs_mut().clear().extend_pairs(&query);

    return_url.to_string()
}
