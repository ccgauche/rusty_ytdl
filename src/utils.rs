use boa_engine::optimizer::OptimizerOptions;
use boa_engine::{Context, JsValue, Source};
use bytes::Bytes;
use once_cell::sync::Lazy;
use rand::Rng;
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::process::Stdio;
use std::sync::Mutex;
use std::time::Instant;
use tokio::sync::RwLock;
use tokio::{io::AsyncWriteExt, process::Command};
use unicode_segmentation::UnicodeSegmentation;
use urlencoding::decode;

use crate::constants::{
    AGE_RESTRICTED_URLS, AUDIO_ENCODING_RANKS, BASE_URL, ESCAPING_SEQUENZES, IPV6_REGEX,
    PARSE_INT_REGEX, VALID_QUERY_DOMAINS, VIDEO_ENCODING_RANKS,
};
use crate::info_extras::{get_author, get_chapters, get_dislikes, get_likes, get_storyboards};
use crate::structs::{
    Embed, EscapeSequence, StringUtils, Thumbnail, VideoDetails, VideoError, VideoFormat,
    VideoOptions, VideoQuality, VideoSearchOptions,
};

#[cfg(feature = "ffmpeg")]
pub async fn ffmpeg_cmd_run(args: &Vec<String>, data: Bytes) -> Result<Bytes, VideoError> {
    use tokio::io::AsyncReadExt;

    let mut cmd = Command::new("ffmpeg");
    cmd.args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .kill_on_drop(true);

    let mut process = cmd.spawn().map_err(|x| VideoError::FFmpeg(x.to_string()))?;
    let mut stdin = process
        .stdin
        .take()
        .ok_or(VideoError::FFmpeg("Failed to open stdin".to_string()))?;

    tokio::spawn(async move { stdin.write_all(&data).await });

    let output = process
        .wait_with_output()
        .await
        .map_err(|x| VideoError::FFmpeg(x.to_string()))?;

    Ok(Bytes::from(output.stdout))
}

#[allow(dead_code)]
pub fn get_cver(info: &serde_json::Value) -> &str {
    info.get("responseContext")
        .and_then(|x| x.get("serviceTrackingParams"))
        .unwrap()
        .as_array()
        .and_then(|x| {
            let index = x
                .iter()
                .position(|r| {
                    r.as_object()
                        .map(|c| c.get("service").unwrap().as_str().unwrap() == "CSI")
                        .unwrap_or(false)
                })
                .unwrap();
            x.get(index)
                .unwrap()
                .as_object()
                .and_then(|x| {
                    let second_array = x.get("params").unwrap().as_array().unwrap();
                    let second_index = second_array
                        .iter()
                        .position(|r| {
                            r.as_object()
                                .map(|c| c.get("key").unwrap().as_str().unwrap() == "cver")
                                .unwrap_or(false)
                        })
                        .unwrap();
                    second_array
                        .get(second_index)
                        .unwrap()
                        .as_object()
                        .unwrap()
                        .get("value")
                })
                .unwrap()
                .as_str()
        })
        .unwrap()
}

#[cfg_attr(feature = "performance_analysis", flamer::flame)]
pub fn get_html5player(body: &str) -> Option<String> {
    // Lazy gain ~0.2ms per req on Ryzen 9 7950XT (probably a lot more on slower CPUs)
    static HTML5PLAYER_RES: Lazy<Regex> = Lazy::new(|| {
        Regex::new(r#"<script\s+src="([^"]+)"(?:\s+type="text\\//javascript")?\s+name="player_ias\\//base"\s*>|"jsUrl":"([^"]+)""#).unwrap()
    });
    let caps = HTML5PLAYER_RES.captures(body).unwrap();
    match caps.get(2) {
        Some(caps) => Some(caps.as_str().to_string()),
        None => match caps.get(3) {
            Some(caps) => Some(caps.as_str().to_string()),
            None => Some(String::from("")),
        },
    }
}


#[cfg_attr(feature = "performance_analysis", flamer::flame)]
pub fn add_format_meta(format: &mut serde_json::Map<String, serde_json::Value>) {
    if format.contains_key("qualityLabel") {
        format.insert("hasVideo".to_owned(), serde_json::Value::Bool(true));
    } else {
        format.insert("hasVideo".to_owned(), serde_json::Value::Bool(false));
    }

    if format.contains_key("audioBitrate") || format.contains_key("audioQuality") {
        format.insert("hasAudio".to_owned(), serde_json::Value::Bool(true));
    } else {
        format.insert("hasAudio".to_owned(), serde_json::Value::Bool(false));
    }

    // 50µs gain/format on Ryzen 9 5950XT (probably a lot more on slower CPUs)
    // Since each request has multiple formats, this saves around 0.5ms per request
    static REGEX_IS_LIVE: Lazy<Regex> =
        Lazy::new(|| Regex::new(r"\bsource[/=]yt_live_broadcast\b").unwrap());
    static REGEX_IS_HLS: Lazy<Regex> =
        Lazy::new(|| Regex::new(r"/manifest/hls_(variant|playlist)/").unwrap());
    static REGEX_IS_DASHMPD: Lazy<Regex> = Lazy::new(|| Regex::new(r"/manifest/dash/").unwrap());

    format.insert(
        "isLive".to_string(),
        serde_json::Value::Bool(
            REGEX_IS_LIVE.is_match(format.get("url").and_then(|x| x.as_str()).unwrap_or("")),
        ),
    );

    format.insert(
        "isHLS".to_string(),
        serde_json::Value::Bool(
            REGEX_IS_HLS.is_match(format.get("url").and_then(|x| x.as_str()).unwrap_or("")),
        ),
    );

    format.insert(
        "isDashMPD".to_string(),
        serde_json::Value::Bool(
            REGEX_IS_DASHMPD.is_match(format.get("url").and_then(|x| x.as_str()).unwrap_or("")),
        ),
    );
}

#[cfg_attr(feature = "performance_analysis", flamer::flame)]
pub fn filter_formats(formats: &mut Vec<VideoFormat>, options: &VideoSearchOptions) {
    match options {
        VideoSearchOptions::Audio => {
            formats.retain(|x| (!x.has_video && x.has_audio) || x.is_live);
        }
        VideoSearchOptions::Video => {
            formats.retain(|x| (x.has_video && !x.has_audio) || x.is_live);
        }
        VideoSearchOptions::Custom(func) => {
            formats.retain(|x| func(x) || x.is_live);
        }
        _ => {
            formats.retain(|x| (x.has_video && x.has_audio) || x.is_live);
        }
    }
}

/// Try to get format with [`VideoOptions`] filter
#[cfg_attr(feature = "performance_analysis", flamer::flame)]
pub fn choose_format<'a>(
    formats: &'a [VideoFormat],
    options: &'a VideoOptions,
) -> Result<VideoFormat, VideoError> {
    let filter = &options.filter;
    let mut formats = formats.to_owned();

    filter_formats(&mut formats, filter);

    if formats.iter().any(|x| x.is_hls) {
        formats.retain(|fmt| (fmt.is_hls) || !(fmt.is_live));
    }

    formats.sort_by(sort_formats);
    match &options.quality {
        VideoQuality::Highest => {
            filter_formats(&mut formats, filter);

            let return_format = formats.first().ok_or(VideoError::FormatNotFound)?;

            Ok(return_format.clone())
        }
        VideoQuality::Lowest => {
            filter_formats(&mut formats, filter);

            let return_format = formats.last().ok_or(VideoError::FormatNotFound)?;

            Ok(return_format.clone())
        }
        VideoQuality::HighestAudio => {
            filter_formats(&mut formats, &VideoSearchOptions::Audio);
            formats.sort_by(sort_formats_by_audio);

            let return_format = formats.first().ok_or(VideoError::FormatNotFound)?;

            Ok(return_format.clone())
        }
        VideoQuality::LowestAudio => {
            filter_formats(&mut formats, &VideoSearchOptions::Audio);

            formats.sort_by(sort_formats_by_audio);

            let return_format = formats.last().ok_or(VideoError::FormatNotFound)?;

            Ok(return_format.clone())
        }
        VideoQuality::HighestVideo => {
            filter_formats(&mut formats, &VideoSearchOptions::Video);
            formats.sort_by(sort_formats_by_video);

            let return_format = formats.first().ok_or(VideoError::FormatNotFound)?;

            Ok(return_format.clone())
        }
        VideoQuality::LowestVideo => {
            filter_formats(&mut formats, &VideoSearchOptions::Video);

            formats.sort_by(sort_formats_by_video);

            let return_format = formats.last().ok_or(VideoError::FormatNotFound)?;

            Ok(return_format.clone())
        }
        VideoQuality::Custom(filter, func) => {
            filter_formats(&mut formats, filter);

            formats.sort_by(|x, y| func(x, y));

            let return_format = formats.first().ok_or(VideoError::FormatNotFound)?;

            Ok(return_format.clone())
        }
    }
}

#[cfg_attr(feature = "performance_analysis", flamer::flame)]
pub fn sort_formats_by<F>(a: &VideoFormat, b: &VideoFormat, sort_by: Vec<F>) -> std::cmp::Ordering
where
    F: Fn(&VideoFormat) -> i32,
{
    let mut res = std::cmp::Ordering::Equal;

    for func in sort_by {
        res = func(b).cmp(&func(a));

        // Is not equal return order
        if res != std::cmp::Ordering::Equal {
            break;
        }
    }

    res
}

#[cfg_attr(feature = "performance_analysis", flamer::flame)]
pub fn sort_formats_by_video(a: &VideoFormat, b: &VideoFormat) -> std::cmp::Ordering {
    sort_formats_by(
        a,
        b,
        [
            |form: &VideoFormat| {
                let quality_label = form.quality_label.clone().unwrap_or("".to_string());

                let quality_label = PARSE_INT_REGEX
                    .captures(&quality_label)
                    .and_then(|x| x.get(0))
                    .map(|x| x.as_str())
                    .and_then(|x| x.parse::<i32>().ok())
                    .unwrap_or(0i32);

                quality_label
            },
            |form: &VideoFormat| form.bitrate as i32,
            // getVideoEncodingRank,
            |form: &VideoFormat| {
                let index = VIDEO_ENCODING_RANKS
                    .iter()
                    .position(|enc| form.mime_type.codecs.join(", ").contains(enc))
                    .map(|x| x as i32)
                    .unwrap_or(-1);

                index
            },
        ]
        .to_vec(),
    )
}

#[cfg_attr(feature = "performance_analysis", flamer::flame)]
pub fn sort_formats_by_audio(a: &VideoFormat, b: &VideoFormat) -> std::cmp::Ordering {
    sort_formats_by(
        a,
        b,
        [
            |form: &VideoFormat| form.audio_bitrate.unwrap_or(0) as i32,
            // getAudioEncodingRank,
            |form: &VideoFormat| {
                let index = AUDIO_ENCODING_RANKS
                    .iter()
                    .position(|enc| form.mime_type.codecs.join(", ").contains(enc))
                    .map(|x| x as i32)
                    .unwrap_or(-1);

                index
            },
        ]
        .to_vec(),
    )
}

#[cfg_attr(feature = "performance_analysis", flamer::flame)]
pub fn sort_formats(a: &VideoFormat, b: &VideoFormat) -> std::cmp::Ordering {
    sort_formats_by(
        a,
        b,
        [
            // Formats with both video and audio are ranked highest.
            |form: &VideoFormat| form.is_hls as i32,
            |form: &VideoFormat| form.is_dash_mpd as i32,
            |form: &VideoFormat| (form.has_video && form.has_audio) as i32,
            |form: &VideoFormat| form.has_video as i32,
            |form: &VideoFormat| {
                (form
                    .content_length
                    .clone()
                    .unwrap_or("0".to_string())
                    .parse::<u64>()
                    .unwrap_or(0)
                    > 0) as i32
            },
            |form: &VideoFormat| {
                let quality_label = form.quality_label.clone().unwrap_or("".to_string());

                let quality_label = PARSE_INT_REGEX
                    .captures(&quality_label)
                    .and_then(|x| x.get(0))
                    .map(|x| x.as_str())
                    .and_then(|x| x.parse::<i32>().ok())
                    .unwrap_or(0i32);

                quality_label
            },
            |form: &VideoFormat| form.bitrate as i32,
            |form: &VideoFormat| form.audio_bitrate.unwrap_or(0) as i32,
            // getVideoEncodingRank,
            |form: &VideoFormat| {
                let index = VIDEO_ENCODING_RANKS
                    .iter()
                    .position(|enc| form.mime_type.codecs.join(", ").contains(enc))
                    .map(|x| x as i32)
                    .unwrap_or(-1);

                index
            },
            // getAudioEncodingRank,
            |form: &VideoFormat| {
                let index = AUDIO_ENCODING_RANKS
                    .iter()
                    .position(|enc| form.mime_type.codecs.join(", ").contains(enc))
                    .map(|x| x as i32)
                    .unwrap_or(-1);

                index
            },
        ]
        .to_vec(),
    )
}

/// Excavate video id from URLs or id with Regex
#[cfg_attr(feature = "performance_analysis", flamer::flame)]
pub fn get_video_id(url: &str) -> Option<String> {
    let url_regex = Regex::new(r"^https?://").unwrap();

    if validate_id(url.to_string()) {
        Some(url.to_string())
    } else if url_regex.is_match(url.trim()) {
        get_url_video_id(url)
    } else {
        None
    }
}

#[cfg_attr(feature = "performance_analysis", flamer::flame)]
pub fn validate_id(id: String) -> bool {
    let id_regex = Regex::new(r"^[a-zA-Z0-9-_]{11}$").unwrap();

    id_regex.is_match(id.trim())
}

#[cfg_attr(feature = "performance_analysis", flamer::flame)]
fn get_url_video_id(url: &str) -> Option<String> {
    // 1ms gain/req on Ryzen 9 5950XT (probably a lot more on slower CPUs)
    static VALID_PATH_DOMAINS: Lazy<Regex> = Lazy::new(|| {
        Regex::new(r"(?m)(?:^|\W)(?:youtube(?:-nocookie)?\.com/(?:.*[?&]v=|v/|shorts/|e(?:mbed)?/|[^/]+/.+/)|youtu\.be/)([\w-]+)")
        .unwrap()
    });

    let parsed_result = url::Url::parse(url.trim());

    if parsed_result.is_err() {
        return None;
    }

    let parsed = url::Url::parse(url.trim()).unwrap();

    let mut id: Option<String> = None;

    for value in parsed.query_pairs() {
        if value.0.to_string().as_str() == "v" {
            id = Some(value.1.to_string());
        }
    }

    if VALID_PATH_DOMAINS.is_match(url.trim()) && id.is_none() {
        let captures = VALID_PATH_DOMAINS.captures(url.trim());
        // println!("{:#?}", captures);
        if let Some(captures_some) = captures {
            let id_group = captures_some.get(1);
            if let Some(id_group_some) = id_group {
                id = Some(id_group_some.as_str().to_string());
            }
        }
    } else if url::Url::parse(url.trim()).unwrap().host_str().is_some()
        && !VALID_QUERY_DOMAINS
            .iter()
            .any(|domain| domain == &parsed.host_str().unwrap_or(""))
    {
        return None;
    }

    if let Some(id_some) = id {
        id = Some(id_some.substring(0, 11).to_string());

        if !validate_id(id.clone().unwrap()) {
            return None;
        }

        id
    } else {
        None
    }
}

#[cfg_attr(feature = "performance_analysis", flamer::flame)]
pub fn get_text(obj: &serde_json::Value) -> &serde_json::Value {
    let null_referance = &serde_json::Value::Null;
    obj.as_object()
        .and_then(|x| {
            if x.contains_key("runs") {
                x.get("runs").and_then(|c| {
                    c.as_array()
                        .unwrap()
                        .first()
                        .and_then(|d| d.as_object().and_then(|f| f.get("text")))
                })
            } else {
                x.get("simpleText")
            }
        })
        .unwrap_or(null_referance)
}

#[cfg_attr(feature = "performance_analysis", flamer::flame)]
pub fn clean_video_details(
    initial_response: &serde_json::Value,
    player_response: &serde_json::Value,
    media: serde_json::Value,
    id: String,
) -> VideoDetails {
    let empty_serde_object = serde_json::json!({});
    let empty_serde_vec: Vec<serde_json::Value> = vec![];
    let empty_serde_map = serde_json::Map::new();

    let mut data = player_response
        .get("microformat")
        .and_then(|x| x.get("playerMicroformatRenderer"))
        .unwrap_or(&empty_serde_object)
        .clone();
    let player_response_video_details = player_response
        .get("videoDetails")
        .unwrap_or(&empty_serde_object)
        .clone();

    // merge two json objects
    merge(&mut data, &player_response_video_details);

    let embed_object = data
        .get("embed")
        .and_then(|x| x.as_object())
        .unwrap_or(&empty_serde_map);
    VideoDetails {
        author: get_author(initial_response, player_response),
        age_restricted: is_age_restricted(&media),

        likes: get_likes(initial_response),
        dislikes: get_dislikes(initial_response),

        video_url: format!("{BASE_URL}{id}"),
        storyboards: get_storyboards(player_response).unwrap_or_default(),
        chapters: get_chapters(initial_response).unwrap_or_default(),

        embed: Embed {
            flash_secure_url: embed_object
                .get("flashSecureUrl")
                .and_then(|x| x.as_str())
                .unwrap_or("")
                .to_string(),
            flash_url: embed_object
                .get("flashUrl")
                .and_then(|x| x.as_str())
                .unwrap_or("")
                .to_string(),
            iframe_url: embed_object
                .get("iframeUrl")
                .and_then(|x| x.as_str())
                .unwrap_or("")
                .to_string(),
            height: embed_object
                .get("height")
                .and_then(|x| {
                    if x.is_string() {
                        x.as_str().map(|x| match x.parse::<i64>() {
                            Ok(a) => a,
                            Err(_err) => 0i64,
                        })
                    } else {
                        x.as_i64()
                    }
                })
                .unwrap_or(0i64) as i32,
            width: embed_object
                .get("width")
                .and_then(|x| {
                    if x.is_string() {
                        x.as_str().map(|x| match x.parse::<i64>() {
                            Ok(a) => a,
                            Err(_err) => 0i64,
                        })
                    } else {
                        x.as_i64()
                    }
                })
                .unwrap_or(0i64) as i32,
        },
        title: data
            .get("title")
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .to_string(),
        description: if data.get("shortDescription").is_some() {
            data.get("shortDescription")
                .and_then(|x| x.as_str())
                .unwrap_or("")
                .to_string()
        } else {
            get_text(data.get("description").unwrap_or(&empty_serde_object))
                .as_str()
                .unwrap_or("")
                .to_string()
        },
        length_seconds: data
            .get("lengthSeconds")
            .and_then(|x| x.as_str())
            .unwrap_or("0")
            .to_string(),
        owner_profile_url: data
            .get("ownerProfileUrl")
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .to_string(),
        external_channel_id: data
            .get("externalChannelId")
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .to_string(),
        is_family_safe: data
            .get("isFamilySafe")
            .and_then(|x| x.as_bool())
            .unwrap_or(false),
        available_countries: data
            .get("availableCountries")
            .and_then(|x| x.as_array())
            .unwrap_or(&empty_serde_vec)
            .iter()
            .map(|x| x.as_str().unwrap_or("").to_string())
            .collect::<Vec<String>>(),
        is_unlisted: data
            .get("isUnlisted")
            .and_then(|x| x.as_bool())
            .unwrap_or(false),
        has_ypc_metadata: data
            .get("hasYpcMetadata")
            .and_then(|x| x.as_bool())
            .unwrap_or(false),
        view_count: data
            .get("viewCount")
            .and_then(|x| x.as_str())
            .unwrap_or("0")
            .to_string(),
        category: data
            .get("category")
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .to_string(),
        publish_date: data
            .get("publishDate")
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .to_string(),
        owner_channel_name: data
            .get("ownerChannelName")
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .to_string(),
        upload_date: data
            .get("uploadDate")
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .to_string(),
        video_id: data
            .get("videoId")
            .and_then(|x| x.as_str())
            .unwrap_or("0")
            .to_string(),
        keywords: data
            .get("keywords")
            .and_then(|x| x.as_array())
            .unwrap_or(&empty_serde_vec)
            .iter()
            .map(|x| x.as_str().unwrap_or("").to_string())
            .collect::<Vec<String>>(),
        channel_id: data
            .get("channelId")
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .to_string(),
        is_owner_viewing: data
            .get("isOwnerViewing")
            .and_then(|x| x.as_bool())
            .unwrap_or(false),
        is_crawlable: data
            .get("isCrawlable")
            .and_then(|x| x.as_bool())
            .unwrap_or(false),
        allow_ratings: data
            .get("allowRatings")
            .and_then(|x| x.as_bool())
            .unwrap_or(false),
        is_private: data
            .get("isPrivate")
            .and_then(|x| x.as_bool())
            .unwrap_or(false),
        is_unplugged_corpus: data
            .get("isUnpluggedCorpus")
            .and_then(|x| x.as_bool())
            .unwrap_or(false),
        is_live_content: data
            .get("isLiveContent")
            .and_then(|x| x.as_bool())
            .unwrap_or(false),
        thumbnails: data
            .get("thumbnail")
            .and_then(|x| x.get("thumbnails"))
            .and_then(|x| x.as_array())
            .unwrap_or(&empty_serde_vec)
            .iter()
            .map(|x| Thumbnail {
                width: x
                    .get("width")
                    .and_then(|x| {
                        if x.is_string() {
                            x.as_str().map(|x| match x.parse::<i64>() {
                                Ok(a) => a,
                                Err(_err) => 0i64,
                            })
                        } else {
                            x.as_i64()
                        }
                    })
                    .unwrap_or(0i64) as u64,
                height: x
                    .get("height")
                    .and_then(|x| {
                        if x.is_string() {
                            x.as_str().map(|x| match x.parse::<i64>() {
                                Ok(a) => a,
                                Err(_err) => 0i64,
                            })
                        } else {
                            x.as_i64()
                        }
                    })
                    .unwrap_or(0i64) as u64,
                url: x
                    .get("url")
                    .and_then(|x| x.as_str())
                    .unwrap_or("")
                    .to_string(),
            })
            .collect::<Vec<Thumbnail>>(),
    }
}

#[cfg_attr(feature = "performance_analysis", flamer::flame)]
pub fn is_verified(badges: &serde_json::Value) -> bool {
    badges
        .as_array()
        .map(|x| {
            let verified_index = x
                .iter()
                .position(|c| {
                    let json = serde_json::json!(c);
                    json["metadataBadgeRenderer"]["tooltip"] == "Verified"
                })
                .unwrap_or(usize::MAX);

            verified_index < usize::MAX
        })
        .unwrap_or(false)
}

#[cfg_attr(feature = "performance_analysis", flamer::flame)]
pub fn is_age_restricted(media: &serde_json::Value) -> bool {
    let mut age_restricted = false;
    if media.is_object() && media.as_object().is_some() {
        age_restricted = AGE_RESTRICTED_URLS.iter().any(|url| {
            media
                .as_object()
                .map(|x| {
                    let mut bool_vec: Vec<bool> = vec![];

                    for (_key, value) in x {
                        if let Some(value_some) = value.as_str() {
                            bool_vec.push(value_some.contains(url))
                        } else {
                            bool_vec.push(false);
                        }
                    }

                    bool_vec.iter().any(|v| v == &true)
                })
                .unwrap_or(false)
        })
    }

    age_restricted
}

#[cfg_attr(feature = "performance_analysis", flamer::flame)]
pub fn is_rental(player_response: &serde_json::Value) -> bool {
    let playability = player_response.get("playabilityStatus");

    if playability.is_none() {
        return false;
    }

    playability
        .and_then(|x| x.get("status"))
        .and_then(|x| x.as_str())
        .unwrap_or("")
        == "UNPLAYABLE"
        && playability
            .and_then(|x| x.get("errorScreen"))
            .and_then(|x| x.get("playerLegacyDesktopYpcOfferRenderer"))
            .is_some()
}

#[cfg_attr(feature = "performance_analysis", flamer::flame)]
pub fn is_not_yet_broadcasted(player_response: &serde_json::Value) -> bool {
    let playability = player_response.get("playabilityStatus");

    if playability.is_none() {
        return false;
    }

    playability
        .and_then(|x| x.get("status"))
        .and_then(|x| x.as_str())
        .unwrap_or("")
        == "LIVE_STREAM_OFFLINE"
}

#[cfg_attr(feature = "performance_analysis", flamer::flame)]
pub fn is_play_error(player_response: &serde_json::Value, statuses: Vec<&str>) -> bool {
    let playability = player_response
        .get("playabilityStatus")
        .and_then(|x| x.get("status").and_then(|x| x.as_str()));

    if let Some(playability_some) = playability {
        return statuses.contains(&playability_some);
    }

    false
}

#[cfg_attr(feature = "performance_analysis", flamer::flame)]
pub fn is_private_video(player_response: &serde_json::Value) -> bool {
    if player_response
        .get("playabilityStatus")
        .and_then(|x| x.get("status"))
        .and_then(|x| x.as_str())
        .unwrap_or("")
        == "LOGIN_REQUIRED"
    {
        return true;
    }

    false
}

// Cache hit reported ~90% of the time with one entry
// 98% of the time with two entries but twice as much memory used (Probably insignificant)
// No gain for the first execution but then ~80ms gain per query on my computer
static FUNCTIONS: Lazy<RwLock<Option<(String, Vec<(String, String)>)>>> =
    Lazy::new(|| RwLock::new(None));

#[cfg_attr(feature = "performance_analysis", flamer::flame)]
pub async fn get_functions(
    html5player: impl Into<String>,
    client: &reqwest_middleware::ClientWithMiddleware,
) -> Result<Vec<(String, String)>, VideoError> {
    let mut url = url::Url::parse(BASE_URL).expect("IMPOSSIBLE");
    url.set_path(&html5player.into());
    url.query_pairs_mut().clear();

    let url = url.as_str();

    {
        // Check if an URL is already cached
        if let Some((cached_url, cached_functions)) = FUNCTIONS.read().await.as_ref() {
            // Check if the cache is the same as the URL
            if cached_url == url {
                return Ok(cached_functions.clone());
            }
        }
    }

    let response = get_html(client, url, None).await?;

    let functions = extract_functions(response);

    // Update the cache
    {
        *FUNCTIONS.write().await = Some((url.to_string(), functions.clone()));
    }

    Ok(functions)
}

#[cfg_attr(feature = "performance_analysis", flamer::flame)]
pub fn extract_functions(body: String) -> Vec<(String, String)> {
    let mut functions: Vec<(String, String)> = vec![];

    #[cfg_attr(feature = "performance_analysis", flamer::flame)]
    fn extract_manipulations(
        body: String,
        caller: &str,
        // cut_after_js_script: &mut js_sandbox::Script,
    ) -> String {
        let function_name = between(caller, r#"a=a.split("");"#, ".");
        if function_name.is_empty() {
            return String::new();
        }

        let function_start = format!(r#"var {function_name}={{"#);
        let ndx = body.find(function_start.as_str());

        if ndx.is_none() {
            return String::new();
        }

        let sub_body = body.slice((ndx.unwrap() + function_start.len() - 1)..);

        // let cut_after_sub_body = cut_after_js_script.call("cutAfterJS", (&sub_body,));
        // let cut_after_sub_body: String = cut_after_sub_body.unwrap_or(String::from("null"));

        let cut_after_sub_body = cut_after_js(sub_body).unwrap_or("null");

        let return_formatted_string = format!("var {function_name}={cut_after_sub_body}");

        return_formatted_string
    }

    #[cfg_attr(feature = "performance_analysis", flamer::flame)]
    fn extract_decipher(
        body: String,
        functions: &mut Vec<(String, String)>,
        // cut_after_js_script: &mut js_sandbox::Script,
    ) {
        let function_name = between(body.as_str(), r#"a.set("alr","yes");c&&(c="#, "(decodeURIC");
        // println!("decipher function name: {}", function_name);
        if !function_name.is_empty() {
            let function_start = format!("{function_name}=function(a)");
            let ndx = body.find(function_start.as_str());

            if let Some(ndx_some) = ndx {
                let sub_body = body.slice((ndx_some + function_start.len())..);

                // let cut_after_sub_body = cut_after_js_script.call("cutAfterJS", (&sub_body,));
                // let cut_after_sub_body: String = cut_after_sub_body.unwrap_or(String::from("{}"));

                let cut_after_sub_body = cut_after_js(sub_body).unwrap_or("{}");

                let mut function_body = format!("var {function_start}{cut_after_sub_body}");

                function_body = format!(
                    "{manipulated_body};{function_body};",
                    manipulated_body = extract_manipulations(
                        body.clone(),
                        function_body.as_str(),
                        // cut_after_js_script
                    ),
                );

                function_body.retain(|c| c != '\n');

                functions.push((function_name.to_string(), function_body));
            }
        }
    }

    #[cfg_attr(feature = "performance_analysis", flamer::flame)]
    fn extract_ncode(
        body: String,
        functions: &mut Vec<(String, String)>,
        // cut_after_js_script: &mut js_sandbox::Script,
    ) {
        let mut function_name = between(body.as_str(), r#"&&(b=a.get("n"))&&(b="#, "(b)");

        let left_name = format!(
            "var {splitted_function_name}=[",
            splitted_function_name = function_name
                .split('[')
                .collect::<Vec<&str>>()
                .first()
                .unwrap_or(&"")
        );

        if function_name.contains('[') {
            function_name = between(body.as_str(), left_name.as_str(), "]");
        }

        // println!("ncode function name: {}", function_name);

        if !function_name.is_empty() {
            let function_start = format!("{function_name}=function(a)");
            let ndx = body.find(function_start.as_str());

            if let Some(ndx_some) = ndx {
                let sub_body = body.slice((ndx_some + function_start.len())..);

                // let cut_after_sub_body = cut_after_js_script.call("cutAfterJS", (&sub_body,));
                // let cut_after_sub_body: String = cut_after_sub_body.unwrap_or(String::from("{}"));

                let cut_after_sub_body = cut_after_js(sub_body).unwrap_or("{}");

                let mut function_body = format!("var {function_start}{cut_after_sub_body};");

                function_body.retain(|c| c != '\n');

                functions.push((function_name.to_string(), function_body));
            }
        }
    }

    extract_decipher(
        body.clone(),
        &mut functions, /*&mut cut_after_js_script*/
    );
    extract_ncode(body, &mut functions /*&mut cut_after_js_script*/);

    // println!("{:#?} {}", functions, functions.len());
    functions
}

pub async fn get_html(
    client: &reqwest_middleware::ClientWithMiddleware,
    url: impl Into<String>,
    headers: Option<&reqwest::header::HeaderMap>,
) -> Result<String, VideoError> {
    let url = url.into();
    #[cfg(feature = "performance_analysis")]
    let _guard = flame::start_guard(format!("get_html {url}"));
    let request = if let Some(some_headers) = headers {
        client.get(url).headers(some_headers.clone())
    } else {
        client.get(url)
    }
    .send()
    .await;

    if request.is_err() {
        return Err(VideoError::ReqwestMiddleware(request.err().unwrap()));
    }

    let response_first = request.unwrap().text().await;

    if response_first.is_err() {
        return Err(VideoError::BodyCannotParsed);
    }

    Ok(response_first.unwrap())
}

/// Try to generate IPv6 with custom valid block
/// # Example
/// ```ignore
/// let ipv6: std::net::IpAddr = get_random_v6_ip("2001:4::/48")?;
/// ```
#[cfg_attr(feature = "performance_analysis", flamer::flame)]
pub fn get_random_v6_ip(ip: impl Into<String>) -> Result<std::net::IpAddr, VideoError> {
    let ipv6_format: String = ip.into();

    if !IPV6_REGEX.is_match(&ipv6_format) {
        return Err(VideoError::InvalidIPv6Format);
    }

    let format_attr = ipv6_format.split('/').collect::<Vec<&str>>();
    let raw_addr = format_attr.first();
    let raw_mask = format_attr.get(1);

    if raw_addr.is_none() || raw_mask.is_none() {
        return Err(VideoError::InvalidIPv6Format);
    }

    let raw_addr = raw_addr.unwrap();
    let raw_mask = raw_mask.unwrap();

    let base_10_mask = raw_mask.parse::<u8>();
    if base_10_mask.is_err() {
        return Err(VideoError::InvalidIPv6Subnet);
    }

    let mut base_10_mask = base_10_mask.unwrap();

    if !(24..=128).contains(&base_10_mask) {
        return Err(VideoError::InvalidIPv6Subnet);
    }

    let base_10_addr = normalize_ip(*raw_addr);
    let mut rng = rand::thread_rng();

    let mut random_addr = [0u16; 8];
    rng.fill(&mut random_addr);

    for (idx, random_item) in random_addr.iter_mut().enumerate() {
        // Calculate the amount of static bits
        let static_bits = std::cmp::min(base_10_mask, 16);
        base_10_mask -= static_bits;
        // Adjust the bitmask with the static_bits
        let mask = (0xffffu32 - ((2_u32.pow((16 - static_bits).into())) - 1)) as u16;
        // Combine base_10_addr and random_item
        let merged = (base_10_addr[idx] & mask) + (*random_item & (mask ^ 0xffff));

        *random_item = merged;
    }

    Ok(std::net::IpAddr::from(random_addr))
}

#[cfg_attr(feature = "performance_analysis", flamer::flame)]
pub fn normalize_ip(ip: impl Into<String>) -> Vec<u16> {
    let ip: String = ip.into();
    let parts = ip
        .split("::")
        .map(|x| x.split(':').collect::<Vec<&str>>())
        .collect::<Vec<Vec<&str>>>();

    let empty_array = vec![];
    let part_start = parts.clone().first().unwrap_or(&empty_array).clone();
    let mut part_end = parts.clone().get(1).unwrap_or(&empty_array).clone();

    part_end.reverse();

    let mut full_ip: Vec<u16> = vec![0, 0, 0, 0, 0, 0, 0, 0];

    for i in 0..std::cmp::min(part_start.len(), 8) {
        full_ip[i] = u16::from_str_radix(part_start[i], 16).unwrap_or(0)
    }

    for i in 0..std::cmp::min(part_end.len(), 8) {
        full_ip[7 - i] = u16::from_str_radix(part_end[i], 16).unwrap_or(0)
    }

    full_ip
}

#[cfg_attr(feature = "performance_analysis", flamer::flame)]
pub fn make_absolute_url(base: &str, url: &str) -> Result<url::Url, VideoError> {
    match url::Url::parse(url) {
        Ok(u) => Ok(u),
        Err(url::ParseError::RelativeUrlWithoutBase) => {
            let base_url = url::Url::parse(base).map_err(VideoError::URLParseError)?;
            Ok(base_url.join(url)?)
        }
        Err(e) => Err(VideoError::URLParseError(e)),
    }
}

#[cfg_attr(feature = "performance_analysis", flamer::flame)]
pub fn time_to_ms(duration: &str) -> usize {
    let mut ms = 0;
    for (i, curr) in duration.split(':').rev().enumerate() {
        ms += curr.parse::<usize>().unwrap_or(0) * (u32::pow(60_u32, i as u32) as usize);
    }
    ms *= 1000;
    ms
}

#[cfg_attr(feature = "performance_analysis", flamer::flame)]
pub fn parse_abbreviated_number(time_str: &str) -> usize {
    let replaced_string = time_str.replace(',', ".").replace(' ', "");
    // Saves 0.1ms per request on Ryzen 9 5950XT (probably a lot more on slower CPUs)
    let string_match_regex: Lazy<Regex> = Lazy::new(|| Regex::new(r"([\d,.]+)([MK]?)").unwrap());
    // let mut return_value = 0usize;

    let caps = string_match_regex
        .captures(replaced_string.as_str())
        .unwrap();

    let return_value = if caps.len() > 0 {
        let mut num;

        match caps.get(1) {
            Some(regex_match) => num = regex_match.as_str().parse::<f32>().unwrap_or(0f32),
            None => num = 0f32,
        }

        let multi = match caps.get(2) {
            Some(regex_match) => regex_match.as_str(),
            None => "",
        };

        match multi {
            "M" => num *= 1000000f32,
            "K" => num *= 1000f32,
            _ => {
                // Do Nothing
            }
        }

        num = num.round();
        num as usize
    } else {
        0usize
    };

    return_value
}

#[cfg_attr(feature = "performance_analysis", flamer::flame)]
pub fn merge(a: &mut serde_json::Value, b: &serde_json::Value) {
    match (a, b) {
        (&mut serde_json::Value::Object(ref mut a), serde_json::Value::Object(b)) => {
            for (k, v) in b {
                merge(a.entry(k.clone()).or_insert(serde_json::Value::Null), v);
            }
        }
        (a, b) => {
            *a = b.clone();
        }
    }
}

#[cfg_attr(feature = "performance_analysis", flamer::flame)]
pub(crate) fn between<'a>(haystack: &'a str, left: &'a str, right: &'a str) -> &'a str {
    let pos: usize;

    if let Some(matched) = haystack.find(left) {
        pos = matched + left.len();
    } else {
        return "";
    }

    let remaining_haystack = &haystack[pos..];

    if let Some(matched) = remaining_haystack.find(right) {
        &haystack[pos..pos + matched]
    } else {
        ""
    }
}

#[cfg_attr(feature = "performance_analysis", flamer::flame)]
// This function uses a state machine architecture and takes around 10µs per request on Ryzen 9 5950XT
// The old function took around 30ms per request on the same CPU
pub fn cut_after_js(mixed_json: &str) -> Option<&str> {
    let bytes = mixed_json.as_bytes();

    // State:
    let mut index = 0;
    let mut nest = 0;
    let mut last_significant: Option<u8> = None;

    // Update function
    while nest > 0 || index == 0 {
        if index >= bytes.len() {
            return None;
        }
        let char = bytes[index];
        match char {
            // Update the nest
            b'{' | b'[' | b'(' => nest += 1,
            b'}' | b']' | b')' => nest -= 1,
            // Skip strings
            b'"' | b'\'' | b'`' => {
                index += 1;
                while bytes[index] != char {
                    if bytes[index] == b'\\' {
                        index += 1;
                    }
                    index += 1;
                }
            }
            // Skip comments
            b'/' if bytes[index + 1] == b'*' => {
                index += 2;
                while !(bytes[index] == b'*' && bytes[index + 1] == b'/') {
                    index += 1;
                }
                index += 2;
                continue;
            }
            // Skip regexes
            b'/' if last_significant
                .as_ref()
                .map(|x| !x.is_ascii_alphanumeric())
                .unwrap_or(false) =>
            {
                index += 1;
                while bytes[index] != char {
                    if bytes[index] == b'\\' {
                        index += 1;
                    }
                    index += 1;
                }
            }
            // Save the last significant character for the regex check
            a if !a.is_ascii_whitespace() => last_significant = Some(a),
            _ => (),
        }
        index += 1;
    }
    if index == 1 {
        return None;
    }
    Some(&mixed_json[0..index])
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cut_after_js() {
        assert_eq!(
            cut_after_js(r#"{"a": 1, "b": 1}"#).unwrap_or(""),
            r#"{"a": 1, "b": 1}"#.to_string()
        );
        println!("[PASSED] test_works_with_simple_json");

        assert_eq!(
            cut_after_js(r#"{"a": 1, "b": 1}abcd"#).unwrap_or(""),
            r#"{"a": 1, "b": 1}"#.to_string()
        );
        println!("[PASSED] test_cut_extra_characters_after_json");

        assert_eq!(
            cut_after_js(r#"{"a": "}1", "b": 1}abcd"#).unwrap_or(""),
            r#"{"a": "}1", "b": 1}"#.to_string()
        );
        println!("[PASSED] test_tolerant_to_double_quoted_string_constants");

        assert_eq!(
            cut_after_js(r#"{"a": '}1', "b": 1}abcd"#).unwrap_or(""),
            r#"{"a": '}1', "b": 1}"#.to_string()
        );
        println!("[PASSED] test_tolerant_to_single_quoted_string_constants");

        let str = "[-1816574795, '\",;/[;', function asdf() { a = 2/3; return a;}]";
        assert_eq!(
            cut_after_js(format!("{}abcd", str).as_str()).unwrap_or(""),
            str.to_string()
        );
        println!("[PASSED] test_tolerant_to_complex_single_quoted_string_constants");

        assert_eq!(
            cut_after_js(r#"{"a": `}1`, "b": 1}abcd"#).unwrap_or(""),
            r#"{"a": `}1`, "b": 1}"#.to_string()
        );
        println!("[PASSED] test_tolerant_to_back_tick_quoted_string_constants");

        assert_eq!(
            cut_after_js(r#"{"a": "}1", "b": 1}abcd"#).unwrap_or(""),
            r#"{"a": "}1", "b": 1}"#.to_string()
        );
        println!("[PASSED] test_tolerant_to_string_constants");

        assert_eq!(
            cut_after_js(r#"{"a": "\"}1", "b": 1}abcd"#).unwrap_or(""),
            r#"{"a": "\"}1", "b": 1}"#.to_string()
        );
        println!("[PASSED] test_tolerant_to_string_with_escaped_quoting");

        assert_eq!(
            cut_after_js(r#"{"a": "\"}1", "b": 1, "c": /[0-9]}}\/}/}abcd"#).unwrap_or(""),
            r#"{"a": "\"}1", "b": 1, "c": /[0-9]}}\/}/}"#.to_string()
        );
        println!("[PASSED] test_tolerant_to_string_with_regexes");

        assert_eq!(
            cut_after_js(r#"{"a": [-1929233002,b,/,][}",],()}(\[)/,2070160835,1561177444]}abcd"#)
                .unwrap_or(""),
            r#"{"a": [-1929233002,b,/,][}",],()}(\[)/,2070160835,1561177444]}"#.to_string()
        );
        println!("[PASSED] test_tolerant_to_string_with_regexes_in_arrays");

        assert_eq!(
            cut_after_js(r#"{"a": "\"}1", "b": 1, "c": [4/6, /[0-9]}}\/}/]}abcd"#).unwrap_or(""),
            r#"{"a": "\"}1", "b": 1, "c": [4/6, /[0-9]}}\/}/]}"#.to_string()
        );
        println!("[PASSED] test_does_not_fail_for_division_followed_by_a_regex");

        assert_eq!(
            cut_after_js(r#"{"a": "\"1", "b": 1, "c": {"test": 1}}abcd"#).unwrap_or(""),
            r#"{"a": "\"1", "b": 1, "c": {"test": 1}}"#.to_string()
        );
        println!("[PASSED] test_works_with_nested_objects");

        let test_str = r#"{"a": "\"1", "b": 1, "c": () => { try { /* do sth */ } catch (e) { a = [2+3] }; return 5}}"#;
        assert_eq!(
            cut_after_js(format!("{}abcd", test_str).as_str()).unwrap_or(""),
            test_str.to_string()
        );
        println!("[PASSED] test_works_with_try_catch");

        assert_eq!(
            cut_after_js(r#"{"a": "\"фыва", "b": 1, "c": {"test": 1}}abcd"#).unwrap_or(""),
            r#"{"a": "\"фыва", "b": 1, "c": {"test": 1}}"#.to_string()
        );
        println!("[PASSED] test_works_with_utf");

        assert_eq!(
            cut_after_js(r#"{"a": "\\\\фыва", "b": 1, "c": {"test": 1}}abcd"#).unwrap_or(""),
            r#"{"a": "\\\\фыва", "b": 1, "c": {"test": 1}}"#.to_string()
        );
        println!("[PASSED] test_works_with_backslashes_in_string");

        assert_eq!(
            cut_after_js(r#"{"text": "\\\\"};"#).unwrap_or(""),
            r#"{"text": "\\\\"}"#.to_string()
        );
        println!("[PASSED] test_works_with_backslashes_towards_end_of_string");

        assert_eq!(
            cut_after_js(r#"[{"a": 1}, {"b": 2}]abcd"#).unwrap_or(""),
            r#"[{"a": 1}, {"b": 2}]"#.to_string()
        );
        println!("[PASSED] test_works_with_array_as_start");

        assert!(cut_after_js("abcd]}").is_none());
        println!("[PASSED] test_returns_error_when_not_beginning_with_bracket");

        assert!(cut_after_js(r#"{"a": 1,{ "b": 1}"#).is_none());
        println!("[PASSED] test_returns_error_when_missing_closing_bracket");
    }
}
