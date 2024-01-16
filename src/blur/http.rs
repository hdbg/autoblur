use anyhow::{anyhow, Result};
use async_trait::async_trait;
use chromiumoxide::{
    cdp::js_protocol::runtime::{CallArgument, CallFunctionOnParams},
    page::Page,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use thiserror;

#[derive(Serialize)]
pub enum RequestMethod {
    Get,
    Post,
    Options,
}

#[derive(Debug, thiserror::Error)]
pub enum RequestError {
    #[error("backend errored")]
    BackendError {
        // #[backtrace]
        #[source]
        source: anyhow::Error,
    },
    #[error("backend didn't return response")]
    NoResponse,
    #[error("invalid backend resp format")]
    InvalidJsonResp {
        #[source]
        source: anyhow::Error,
    },
}

pub type Headers = HashMap<String, String>;
#[derive(Serialize)]
pub struct Request {
    url: String,
    method: RequestMethod,
    body: Option<String>,
    headers: Headers,
}

impl Request {
    pub fn builder(url: &str) -> RequestBuilder {
        RequestBuilder {
            url: url.to_string(),
            method: None,
            body: None,
            headers: HashMap::new(),
        }
    }
}

pub struct RequestBuilder {
    url: String,
    method: Option<RequestMethod>,
    body: Option<String>,
    headers: Headers,
}

#[derive(Deserialize, Debug)]
pub struct Response {
    pub status: u32,
    pub body: Option<String>,
    pub headers: Headers,
}

impl RequestBuilder {
    pub fn new(url: &str) -> RequestBuilder {
        RequestBuilder {
            url: url.to_string(),
            method: None,
            body: None,
            headers: HashMap::new(),
        }
    }

    pub fn with_method(mut self, method: RequestMethod) -> Self {
        self.method = Some(method);
        self
    }

    pub fn with_body<T: ToString>(mut self, body: T) -> Self {
        self.body = Some(body.to_string());
        self
    }

    pub fn with_header(mut self, key: &str, value: &str) -> Self {
        self.headers.insert(key.to_string(), value.to_string());
        self
    }

    pub fn build(self) -> Request {
        Request {
            url: self.url,
            method: self.method.unwrap(),
            body: self.body,
            headers: self.headers,
        }
    }
}

#[async_trait]
pub trait RequestBackend {
    async fn send(&mut self, req: Request) -> Result<Response>;
}

#[async_trait]
impl RequestBackend for Page {
    async fn send(&mut self, req: Request) -> Result<Response> {
        const REQ_SCRIPT: &str = "
        async function http_fetch(arg) {
            const realArg = JSON.parse(arg);
            console.log(realArg);
            const resp = await fetch(realArg.url, {
                headers: realArg.headers,
                method: realArg.method.toUpperCase(),
                body: realArg.body,
                credentials: \"include\"
            })

            var rheaders = {};

            for (const [key, val] of resp.headers.entries()) {
                rheaders[key] = val;
            }

            var res = {
                status: resp.status,
                headers: rheaders
            }

            if (resp.body != null) {
                res.body = await resp.text();
            }

            return JSON.stringify(res);
        }
    ";

        let req = serde_json::to_string(&req)?;

        let script = CallFunctionOnParams::builder()
            .function_declaration(REQ_SCRIPT)
            .argument(CallArgument::builder().value(req).build())
            .await_promise(true)
            .build()
            .map_err(|x| anyhow!(x))?;

        let script_result = self
            .evaluate_function(script)
            .await
            .map_err(|x| RequestError::BackendError { source: x.into() })?;

        let Value::String(script_result) = script_result.value().unwrap() else {return Err(RequestError::NoResponse.into())};

        let response: Response = serde_json::from_str(script_result)
            .map_err(|x| RequestError::InvalidJsonResp { source: x.into() })?;

        Ok(response)
    }
}
