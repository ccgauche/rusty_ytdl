use std::collections::HashMap;

use boa_engine::{Context, JsValue, Source};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use urlencoding::decode;

use crate::{utils::add_format_meta, VideoFormat};

mod cipher;
mod ncode;

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
            let parsed: SetDownloadURLValue = serde_json::from_value(format.clone()).unwrap();
            format.as_object_mut().map(|x| {
                let new_url = set_download_url(
                    &parsed,
                    format_functions.clone(),
                    &mut n_transform_cache,
                    &mut cipher_cache,
                );

                // Delete unnecessary cipher, signatureCipher
                x.remove("signatureCipher");
                x.remove("cipher");

                x.insert("url".to_string(), Value::String(new_url));

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

#[derive(Deserialize)]
pub struct SetDownloadURLValue {
    url: Option<String>,
    #[serde(rename = "signatureCipher")]
    signature_cipher: Option<String>,
    cipher: Option<String>,
}

#[cfg_attr(feature = "performance_analysis", flamer::flame)]
pub fn set_download_url(
    format: &SetDownloadURLValue,
    functions: Vec<(String, String)>,
    n_transform_cache: &mut HashMap<String, String>,
    cipher_cache: &mut Option<(String, Context)>,
) -> String {

    let empty_script = ("".to_string(), "".to_string());
    let decipher_script_string = functions.first().unwrap_or(&empty_script);
    let n_transform_script_string = functions.get(1).unwrap_or(&empty_script);

    let cipher = format.url.is_none();
    let url = format.url.clone().unwrap_or(
        format
            .signature_cipher
            .clone()
            .unwrap_or(format.cipher.clone().unwrap_or_default()),
    );

    if cipher {
        ncode::ncode(
            cipher::decipher(&url, decipher_script_string, cipher_cache).as_str(),
            n_transform_script_string,
            n_transform_cache,
        )
    } else {
        ncode::ncode(&url, n_transform_script_string, n_transform_cache)
    }
}
