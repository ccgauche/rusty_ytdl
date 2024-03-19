use std::collections::HashMap;

use boa_engine::{Context, JsValue, Source};
use serde::{Deserialize, Serialize};
use urlencoding::decode;

use crate::{utils::add_format_meta, VideoFormat};

mod ncode;
mod cipher;


#[cfg_attr(feature = "performance_analysis", flamer::flame)]
pub fn parse_video_formats(
    info: &serde_json::Value,
    format_functions: Vec<(String, String)>,
) -> Option<Vec<VideoFormat>> {
    if info.as_object()?.contains_key("streamingData") {
        let formats = info
            .as_object()?
            .get("streamingData")
            .and_then(|x| x.get("formats"))?
            .as_array()?;
        let adaptive_formats = info
            .as_object()?
            .get("streamingData")
            .and_then(|x| x.get("adaptiveFormats"))?
            .as_array()?;
        let mut formats = [&formats[..], &adaptive_formats[..]].concat();

        let mut n_transform_cache: HashMap<String, String> = HashMap::new();

        let mut cipher_cache: Option<(String, Context)> = None;
        for format in &mut formats {
            format.as_object_mut().map(|x| {
                let new_url = set_download_url(
                    &mut serde_json::json!(x),
                    format_functions.clone(),
                    &mut n_transform_cache,
                    &mut cipher_cache,
                );

                // Delete unnecessary cipher, signatureCipher
                x.remove("signatureCipher");
                x.remove("cipher");

                x.insert("url".to_string(), new_url);

                // Add Video metaData
                add_format_meta(x);

                x
            });
        }

        let mut well_formated_formats: Vec<VideoFormat> = vec![];

        // Change formats type serde_json::Value to VideoFormat
        for format in formats.iter() {
            let well_formated_format: Result<VideoFormat, serde_json::Error> =
                serde_json::from_value(format.clone());

            if well_formated_format.is_err() {
                continue;
            }

            well_formated_formats
                .insert(well_formated_formats.len(), well_formated_format.unwrap());
        }

        Some(well_formated_formats)
    } else {
        None
    }
}

#[cfg_attr(feature = "performance_analysis", flamer::flame)]
pub fn set_download_url(
    format: &mut serde_json::Value,
    functions: Vec<(String, String)>,
    n_transform_cache: &mut HashMap<String, String>,
    cipher_cache: &mut Option<(String, Context)>,
) -> serde_json::Value {
    let empty_string_serde_value = serde_json::json!("");
    #[derive(Debug, Deserialize, PartialEq, Serialize)]
    struct Query {
        n: String,
        url: String,
        s: String,
        sp: String,
    }

    let empty_script = ("".to_string(), "".to_string());
    let decipher_script_string = functions.first().unwrap_or(&empty_script);
    let n_transform_script_string = functions.get(1).unwrap_or(&empty_script);

    let return_format = format.as_object_mut().unwrap();

    let cipher = return_format.get("url").is_none();
    let url = return_format
        .get("url")
        .unwrap_or(
            return_format.get("signatureCipher").unwrap_or(
                return_format
                    .get("cipher")
                    .unwrap_or(&empty_string_serde_value),
            ),
        )
        .as_str()
        .unwrap_or("");

    if cipher {
        return_format.insert(
            "url".to_string(),
            serde_json::json!(&ncode::ncode(
                cipher::decipher(url, decipher_script_string, cipher_cache).as_str(),
                n_transform_script_string,
                n_transform_cache
            )),
        );
    } else {
        return_format.insert(
            "url".to_string(),
            serde_json::json!(&ncode::ncode(url, n_transform_script_string, n_transform_cache)),
        );
    }

    // Delete unnecessary cipher, signatureCipher
    return_format.remove("signatureCipher");
    return_format.remove("cipher");

    let return_url = url::Url::parse(
        return_format
            .get("url")
            .and_then(|x| x.as_str())
            .unwrap_or(""),
    )
    .unwrap();

    serde_json::json!(return_url.to_string())
}