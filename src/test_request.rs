use assert_json_diff::assert_json_matches_no_panic;
use bytes::Bytes;
use core::iter::Peekable;
use http::request::Request;
use http::Version;
use http_body_util::{BodyExt, Empty};
use hyper::{body::Buf, StatusCode};
use serde_json::Value;
use std::slice::Iter;
use std::{collections::HashMap, fs::read_to_string};
use tokio::net::TcpStream;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

#[derive(Debug, Clone, Copy)]
enum ResultType {
    Json,
    Xml,
    None,
}

#[derive(Debug, Clone)]
pub struct TestRequest {
    id: Option<usize>,
    status: Option<String>,
    method: String,
    url: hyper::Uri,
    expected_result: (ResultType, String),
    version: Option<Version>,
    json_data: Option<Value>,
    headers: Option<Vec<(String, String)>>,
}

impl TestRequest {
    pub async fn test(&self) -> Result<Option<String>> {
        let (val1, status) = self.fetch_res().await?;

        let val2: Value = serde_json::from_str(read_to_string(self.expected_result.1.clone())?.as_str())?;

        let config = assert_json_diff::Config::new(assert_json_diff::CompareMode::Strict);

        if let Err(err) = assert_json_matches_no_panic(&val1, &val2, config) {
            // TODO   println!("{}", serde_json::to_string_pretty(&obj).unwrap());
            return Ok(Some(format!("{err}")));
        }

        if let Some(s) = self.status.clone() {
            if s != status.as_str() {
                return Ok(Some(format!(
                    "http status dose not match expected {s}, but got {status}"
                )));
            }
        }
        return Ok(None);
    }

    async fn fetch_res(&self) -> Result<(Value, StatusCode)> {
        let host = self.url.host().expect("uri has no host");
        let port = self.url.port_u16().unwrap_or(80);
        let addr = format!("{}:{}", host, port);

        let stream = TcpStream::connect(addr).await?;

        let (mut sender, conn) = hyper::client::conn::http1::handshake(stream).await?;
        tokio::task::spawn(async move {
            if let Err(err) = conn.await {
                println!("Connection failed: {:?}", err);
            }
        });

        let authority = self.url.authority().unwrap().clone();

        let req = Request::builder()
            .version(Version::HTTP_2)
            .method(self.method.as_str())
            .uri(self.url.clone())
            .header(hyper::header::HOST, authority.as_str())
            .body(Empty::<Bytes>::new())?;

        let res = sender.send_request(req).await?;
        let status = res.status().to_owned();

        let body = res.collect().await?.aggregate();

        Ok((serde_json::from_reader(body.reader())?, status))
    }

    fn parse_http(lines: &mut Peekable<Iter<String>>, line: String, path: String) -> Result<Self> {
        if !line.starts_with("###") {
            return Err("unexpected character expected '###'".into());
        }

        let line = line[3..].trim().to_string();

        let comment = parse_comment(line);

        let (method, url, version) = parse_req(lines.next().unwrap().to_owned());

        let headers = parse_headers(lines);

        let json_data = parse_json_data(lines)?;

        let id: Result<Option<usize>> = if let Some(id) = comment.get("id") {
            match id.parse::<usize>() {
                Err(err) => Err(err.into()),
                Ok(id) => Ok(Some(id)),
            }
        } else {
            Ok(None)
        };
        let id = id?;

        let status = comment.get("status").map(|s| s.to_string());

        let result_type = comment.get("result_type").map(|s| s.to_string());
        let result_type = if let Some(result_type) = result_type {
            match result_type.as_str() {
                "json" => ResultType::Json,
                "xml" => ResultType::Xml,
                _ => ResultType::None,
            }
        } else {
            ResultType::None
        };

        let mut result_path = comment.get("result_path").map(|s| s.to_string());

        if result_path.is_none() {
            let err = Err("no output result is defined".into());

            if id == None {
                return err;
            }

            let paths = std::fs::read_dir(path.clone());
            if paths.is_err() {
                return err;
            }

            for files in paths.unwrap() {
                if files.is_err() {
                    continue;
                }
                let files = files.unwrap().file_name().into_string();
                if files.is_err() {
                    continue;
                }
                let files = files
                    .unwrap()
                    .splitn(2, '.')
                    .map(|s| s.to_string())
                    .collect::<Vec<_>>();

                if files[0] == id.unwrap().to_string() {
                    result_path = Some(format!("{path}/{}.{}", id.unwrap(), files[1]));
                    break;
                }
            }

            if result_path.is_none() {
                return err;
            }
        }

        Ok(Self {
            id,
            status,
            method,
            url: url.parse()?,
            version,
            json_data,
            headers,
            expected_result: (result_type, result_path.unwrap()),
        })
    }

    pub fn parse_http_file(path: String) -> Result<Vec<Self>> {
        let content = read_to_string(path.clone())?;

        let path = path
            .splitn(2, '.')
            .map(|s| s.to_string())
            .collect::<Vec<_>>();
        if path[1] != "http" {
            return Err("expected file ending to be http".into());
        }

        let lines = content
            .lines()
            .map(|line| line.trim().to_string())
            .collect::<Vec<String>>();

        let mut lines = lines.iter().peekable();

        let mut result = Vec::new();

        while let Some(line) = lines.next() {
            if line.starts_with("# ") {
                continue;
            }

            result.push(Self::parse_http(
                &mut lines,
                line.to_owned(),
                path[0].to_string(),
            )?);
        }

        Ok(result)
    }
}
fn parse_json_data(lines: &mut Peekable<Iter<String>>) -> Result<Option<Value>> {
    let mut json_str: Vec<String> = Vec::new();

    while let Some(line) = lines.peek() {
        if line.starts_with("#") {
            return if json_str.is_empty() {
                Ok(None)
            } else {
                let json_str = json_str.join("");
                Ok(Some(serde_json::from_str(&json_str)?))
            };
        } else {
            json_str.push(lines.next().unwrap().to_owned());
        }
    }
    return Ok(None);
}

fn parse_headers(lines: &mut Peekable<Iter<String>>) -> Option<Vec<(String, String)>> {
    let mut headers = Vec::new();

    while let Some(line) = lines.next() {
        if line.is_empty() {
            return if headers.is_empty() {
                None
            } else {
                Some(headers)
            };
        }
        let line = line
            .splitn(2, ":")
            .map(|s| s.trim().to_string())
            .collect::<Vec<String>>();
        assert_eq!(line.len(), 2);

        headers.push((line[0].to_owned(), line[1].to_owned()));
    }
    return None;
}
fn parse_req(req: String) -> (String, String, Option<Version>) {
    let req = req
        .split(" ")
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect::<Vec<String>>();

    let method = req[0].to_owned();
    let url = req[1].to_owned();

    let version = if req.len() > 2 {
        match req[2].to_owned().as_str() {
            "HTTP/1.1" => Some(Version::HTTP_11),
            _ => None,
        }
    } else {
        None
    };

    return (method, url, version);
}

fn parse_comment(comment: String) -> HashMap<String, String> {
    let mut result = HashMap::new();
    let comments = comment
        .split(",")
        .map(|s| s.trim().to_string())
        .collect::<Vec<String>>();

    for comment in comments {
        let comment = comment
            .splitn(2, ":")
            .map(|s| s.trim().to_string())
            .collect::<Vec<String>>();
        assert_eq!(comment.len(), 2);

        result.insert(comment[0].to_owned(), comment[1].to_owned());
    }
    return result;
}
