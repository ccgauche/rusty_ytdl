use boa_engine::Context;
use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct DecipherQuery {
    url: String,
    s: String,
    // Default value is "signature"
    #[serde(default = "default_signature")]
    sp: String,
}

fn default_signature() -> String {
    "signature".into()
}

#[cfg_attr(feature = "performance_analysis", flamer::flame)]
fn decode_url(url: &str) -> Option<DecipherQuery> {
    serde_qs::from_str(url).ok()
}

#[cfg_attr(feature = "performance_analysis", flamer::flame)]
fn execute_script(context: &mut Context, func_name: &str, s: &str) -> String {
    context
        .eval(boa_engine::Source::from_bytes(&format!(
            r#"{func_name}("{s}")"#
        )))
        .expect("Can't execute script")
        .as_string()
        .expect("Can't convert to string")
        .to_std_string()
        .expect("Can't convert to string")
}

#[cfg_attr(feature = "performance_analysis", flamer::flame)]
fn create_cipher_context<'a, 'b>(script: &'a str) -> Context<'b> {
    let mut context = boa_engine::Context::default();
    context
        .eval(boa_engine::Source::from_bytes(script))
        .expect("Can't execute script");
    context
}

fn get_cipher_context_and_execute(
    decipher_script_string: &(String, String),
    args: &DecipherQuery,
    cipher_cache: &mut Option<(String, Context)>,
) -> String {
    let context = match cipher_cache {
        Some((ref cache_key, ref mut context)) if cache_key == &decipher_script_string.1 => context,
        _ => {
            let context = create_cipher_context(&decipher_script_string.1);
            *cipher_cache = Some((decipher_script_string.1.to_string(), context));
            &mut cipher_cache.as_mut().unwrap().1
        }
    };

    execute_script(context, decipher_script_string.0.as_str(), &args.s)
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

    let convert_result_to_rust_string =
        get_cipher_context_and_execute(decipher_script_string, &args, cipher_cache);

    let mut return_url = url::Url::parse(&args.url).expect("Can't parse the url");

    // Removes the query parameter if it exists and appends the new one
    let mut query = return_url
        .query_pairs()
        .filter(|(name, _)| name.as_ref() != args.sp)
        .map(|(name, value)| (name.into_owned(), value.into_owned()))
        .collect::<Vec<(String, String)>>();
    query.push((args.sp, convert_result_to_rust_string));

    return_url.query_pairs_mut().clear().extend_pairs(&query);

    return_url.to_string()
}
