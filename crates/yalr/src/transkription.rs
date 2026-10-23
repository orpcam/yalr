//! Spracherkennung (`/v1/audio/transcriptions`, OpenAI-kompatibel).
//!
//! Die Anfrage kommt als `multipart/form-data` (Datei + Felder wie `model`,
//! `language`, `response_format`). Am Eingang wird sie in das JSON übersetzt,
//! mit dem die übrige Pipeline arbeitet (Schlüssel, Modellrechte, Routing,
//! Fallback, Protokoll): Felder als Text, die Datei als Data-URL unter `file`
//! – die lässt das Protokoll ohnehin weg (`<… bytes omitted>`). Erst beim
//! Provider wird daraus wieder Multipart, mit dem Upstream-Modellnamen.
//! Unbekannte Felder (z. B. eigene Erweiterungen eines Providers) gehen
//! unverändert mit.

use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use serde_json::{json, Map, Value};

pub const ENDPOINT: &str = "/v1/audio/transcriptions";

/// Größte Aufnahme je Anfrage (eine längere Aufnahme als MP3, mit Reserve).
pub const MAX_UPLOAD_BYTES: usize = 64 * 1024 * 1024;

/// Multipart → internes JSON. Fehler als Text für die 400-Antwort.
pub async fn als_json(content_type: &str, body: bytes::Bytes) -> Result<Value, String> {
    let boundary = multer::parse_boundary(content_type).map_err(|e| format!("expected multipart/form-data: {e}"))?;
    let strom = futures::stream::once(async move { Ok::<_, std::io::Error>(body) });
    let mut teile = multer::Multipart::new(strom, boundary);
    let mut o = Map::new();
    while let Some(feld) = teile.next_field().await.map_err(|e| format!("invalid multipart: {e}"))? {
        let name = feld.name().unwrap_or_default().to_string();
        if name == "file" {
            let mime = feld.content_type().map(|m| m.to_string()).unwrap_or_else(|| "application/octet-stream".into());
            let datei = feld.file_name().unwrap_or("audio").to_string();
            let daten = feld.bytes().await.map_err(|e| format!("invalid multipart: {e}"))?;
            o.insert("file".into(), json!(format!("data:{mime};base64,{}", STANDARD.encode(&daten))));
            o.insert("filename".into(), json!(datei));
        } else if !name.is_empty() {
            let text = feld.text().await.map_err(|e| format!("invalid multipart: {e}"))?;
            o.insert(name, json!(text));
        }
    }
    if !o.contains_key("file") {
        return Err("multipart request has no 'file' field".into());
    }
    Ok(Value::Object(o))
}

/// Internes JSON → Multipart für den Provider; `model` wird zum Upstream-Modell.
pub fn als_form(req_json: &Value, upstream_model: &str) -> Result<reqwest::multipart::Form, String> {
    let o = req_json.as_object().ok_or("transcription request is not an object")?;
    let url = o.get("file").and_then(Value::as_str).ok_or("transcription request has no file")?;
    let (kopf, nutzlast) = url
        .strip_prefix("data:")
        .and_then(|r| r.split_once(";base64,"))
        .ok_or("file is not a data URL")?;
    let daten = STANDARD.decode(nutzlast).map_err(|e| format!("file is not base64: {e}"))?;
    let datei = o.get("filename").and_then(Value::as_str).unwrap_or("audio").to_string();
    let teil = reqwest::multipart::Part::bytes(daten)
        .file_name(datei)
        .mime_str(kopf)
        .map_err(|e| format!("invalid content type: {e}"))?;
    let mut form = reqwest::multipart::Form::new().part("file", teil);
    for (k, v) in o {
        match k.as_str() {
            "file" | "filename" => {}
            "model" => form = form.text("model", upstream_model.to_string()),
            _ => {
                let text = match v {
                    Value::String(s) => s.clone(),
                    anders => anders.to_string(),
                };
                form = form.text(k.clone(), text);
            }
        }
    }
    Ok(form)
}

/// Was von der Antwort ins Protokoll kommt: JSON wie geliefert, reiner Text als `{"text": …}`.
pub fn antwort_fuers_log(body: &[u8]) -> Value {
    serde_json::from_slice(body).unwrap_or_else(|_| json!({ "text": String::from_utf8_lossy(body) }))
}

#[cfg(test)]
pub(crate) fn multipart_fuer_test(felder: &[(&str, &str)], datei: &[u8]) -> (String, bytes::Bytes) {
    let grenze = "----yalr-test-grenze";
    let mut b = Vec::new();
    for (k, v) in felder {
        b.extend_from_slice(format!("--{grenze}\r\nContent-Disposition: form-data; name=\"{k}\"\r\n\r\n{v}\r\n").as_bytes());
    }
    b.extend_from_slice(
        format!("--{grenze}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"audio.mp3\"\r\nContent-Type: audio/mpeg\r\n\r\n")
            .as_bytes(),
    );
    b.extend_from_slice(datei);
    b.extend_from_slice(format!("\r\n--{grenze}--\r\n").as_bytes());
    (format!("multipart/form-data; boundary={grenze}"), bytes::Bytes::from(b))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_als_json_felder_und_datei() {
        let (ct, body) = multipart_fuer_test(
            &[("model", "whisper-1"), ("language", "de"), ("prompt", "Ortsnamen")],
            &[0u8, 1, 2, 255],
        );
        let v = als_json(&ct, body).await.unwrap();
        assert_eq!(v["model"], "whisper-1");
        assert_eq!(v["language"], "de");
        assert_eq!(v["prompt"], "Ortsnamen");
        assert_eq!(v["filename"], "audio.mp3");
        assert_eq!(v["file"], format!("data:audio/mpeg;base64,{}", STANDARD.encode([0u8, 1, 2, 255])));
    }

    #[tokio::test]
    async fn test_als_json_fehler() {
        assert!(als_json("application/json", bytes::Bytes::from_static(b"{}")).await.is_err());
        let (ct, _) = multipart_fuer_test(&[], b"");
        let ohne_datei = format!("--{}--\r\n", ct.split("boundary=").nth(1).unwrap());
        let e = als_json(&ct, bytes::Bytes::from(ohne_datei)).await.unwrap_err();
        assert!(e.contains("no 'file'"), "{e}");
    }

    #[test]
    fn test_als_form_braucht_data_url() {
        assert!(als_form(&json!({"model": "m", "file": "data:audio/mpeg;base64,AAE="}), "up").is_ok());
        assert!(als_form(&json!({"model": "m", "file": "https://x/y.mp3"}), "up").is_err());
        assert!(als_form(&json!({"model": "m"}), "up").is_err());
        assert!(als_form(&json!({"model": "m", "file": "data:audio/mpeg;base64,!!"}), "up").is_err());
    }

    #[test]
    fn test_antwort_fuers_log() {
        assert_eq!(antwort_fuers_log(br#"{"text":"Hallo"}"#), json!({"text": "Hallo"}));
        assert_eq!(antwort_fuers_log(b"Hallo Welt"), json!({"text": "Hallo Welt"}));
    }
}
