use std::{borrow::Cow, collections::HashMap};

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

fn extract_n_from_url(url: &Url) -> Option<Cow<str>> {
    url.query_pairs()
        .find(|(name, _)| name == "n")
        .map(|(_, v)| v)
}

fn apply_transform(
    n_transform_script_string: &(String, String),
    n_transfrom_cache: &mut HashMap<String, String>,
    n: &str,
) -> String {
    let mut context = create_transform_script(n_transform_script_string.1.as_str());

    let is_result_string =
        execute_transform_script(&mut context, n_transform_script_string.0.as_str(), &n);

    let is_result_string = is_result_string
        .as_string()
        .expect("Can't convert to string");
    let convert_result_to_rust_string = is_result_string
        .to_std_string()
        .expect("Can't convert to string");

    n_transfrom_cache.insert(n.to_owned(), convert_result_to_rust_string.clone());

    convert_result_to_rust_string
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

    let query = return_url
        .query_pairs()
        .map(|(name, value)| {
            if name != "n" {
                return (name.into_owned(), value.into_owned());
            }
            (
                name.into_owned(),
                n_transfrom_cache
                    .get(value.as_ref())
                    .cloned()
                    .unwrap_or_else(|| {
                        apply_transform(
                            n_transform_script_string,
                            n_transfrom_cache,
                            value.as_ref(),
                        )
                    }),
            )
        })
        .collect::<Vec<_>>();

    return_url.query_pairs_mut().clear().extend_pairs(&query);

    return_url.to_string()
}
