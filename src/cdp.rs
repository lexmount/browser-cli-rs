use std::{
    collections::VecDeque,
    fs,
    net::TcpStream,
    path::Path,
    time::{Duration, Instant},
};

use base64::{Engine, engine::general_purpose::STANDARD};
use serde_json::{Value, json};
use tungstenite::{Message, WebSocket, stream::MaybeTlsStream};

use crate::{Error, Result};

pub struct Cdp {
    socket: WebSocket<MaybeTlsStream<TcpStream>>,
    next_id: u64,
    target_session_id: String,
    events: VecDeque<Value>,
}

impl Cdp {
    pub fn connect(url: &str) -> Result<Self> {
        let (socket, _) = tungstenite::connect(url)?;
        let mut client = Self {
            socket,
            next_id: 1,
            target_session_id: String::new(),
            events: VecDeque::new(),
        };
        let targets = client.command_root("Target.getTargets", json!({}))?;
        let target_id = targets
            .get("targetInfos")
            .and_then(Value::as_array)
            .and_then(|items| {
                items
                    .iter()
                    .find(|v| v.get("type").and_then(Value::as_str) == Some("page"))
            })
            .and_then(|v| v.get("targetId"))
            .and_then(Value::as_str)
            .map(str::to_owned);
        let target_id = match target_id {
            Some(id) => id,
            None => client
                .command_root("Target.createTarget", json!({"url":"about:blank"}))?
                .get("targetId")
                .and_then(Value::as_str)
                .ok_or_else(|| Error::Cdp("Target.createTarget response missing targetId".into()))?
                .to_owned(),
        };
        let attached = client.command_root(
            "Target.attachToTarget",
            json!({"targetId": target_id, "flatten": true}),
        )?;
        client.target_session_id = attached
            .get("sessionId")
            .and_then(Value::as_str)
            .ok_or_else(|| Error::Cdp("Target.attachToTarget response missing sessionId".into()))?
            .to_owned();
        client.command("Page.enable", json!({}))?;
        client.command("Runtime.enable", json!({}))?;
        Ok(client)
    }

    pub fn command(&mut self, method: &str, params: Value) -> Result<Value> {
        let session_id = self.target_session_id.clone();
        self.send(method, params, Some(&session_id))
    }

    pub fn command_root(&mut self, method: &str, params: Value) -> Result<Value> {
        self.send(method, params, None)
    }

    fn send(&mut self, method: &str, params: Value, session_id: Option<&str>) -> Result<Value> {
        let id = self.next_id;
        self.next_id += 1;
        let mut request = json!({"id": id, "method": method, "params": params});
        if let Some(session_id) = session_id {
            request["sessionId"] = Value::String(session_id.into());
        }
        self.socket
            .send(Message::Text(request.to_string().into()))?;
        loop {
            let raw = self.socket.read()?;
            if !raw.is_text() {
                continue;
            }
            let message: Value = serde_json::from_str(raw.to_text()?)?;
            if message.get("id").and_then(Value::as_u64) == Some(id) {
                if let Some(error) = message.get("error") {
                    return Err(Error::Cdp(
                        error
                            .get("message")
                            .and_then(Value::as_str)
                            .unwrap_or("unknown CDP error")
                            .into(),
                    ));
                }
                return Ok(message.get("result").cloned().unwrap_or(Value::Null));
            }
            if message.get("method").is_some() {
                self.events.push_back(message);
            }
        }
    }

    pub fn navigate(&mut self, url: &str, timeout: Duration) -> Result<Value> {
        let result = self.command("Page.navigate", json!({"url": url}))?;
        let started = Instant::now();
        loop {
            let state = self.evaluate("document.readyState")?;
            if state
                .as_str()
                .is_some_and(|v| v == "interactive" || v == "complete")
            {
                break;
            }
            if started.elapsed() >= timeout {
                return Err(Error::Timeout(format!("navigation to {url}")));
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        Ok(
            json!({"navigation": result, "url": self.evaluate("location.href")?, "title": self.evaluate("document.title")?}),
        )
    }

    pub fn evaluate(&mut self, expression: &str) -> Result<Value> {
        let result = self.command(
            "Runtime.evaluate",
            json!({"expression": expression, "awaitPromise": true, "returnByValue": true}),
        )?;
        if let Some(exception) = result.get("exceptionDetails") {
            return Err(Error::Cdp(
                exception
                    .get("text")
                    .and_then(Value::as_str)
                    .unwrap_or("JavaScript evaluation failed")
                    .into(),
            ));
        }
        Ok(result
            .get("result")
            .and_then(|v| v.get("value"))
            .cloned()
            .unwrap_or(Value::Null))
    }

    pub fn click(&mut self, selector: &str) -> Result<Value> {
        self.evaluate(&format!("(()=>{{const e=document.querySelector({});if(!e)throw new Error('selector not found');e.scrollIntoView({{block:'center'}});e.click();return true}})()", serde_json::to_string(selector)?))
    }

    pub fn fill(&mut self, selector: &str, value: &str) -> Result<Value> {
        self.evaluate(&format!("(()=>{{const e=document.querySelector({});if(!e)throw new Error('selector not found');const s=Object.getOwnPropertyDescriptor(Object.getPrototypeOf(e),'value')?.set;s?s.call(e,{}):e.value={};e.dispatchEvent(new Event('input',{{bubbles:true}}));e.dispatchEvent(new Event('change',{{bubbles:true}}));return true}})()", serde_json::to_string(selector)?, serde_json::to_string(value)?, serde_json::to_string(value)?))
    }

    pub fn wait_selector(&mut self, selector: &str, timeout: Duration) -> Result<Value> {
        let started = Instant::now();
        loop {
            let found = self.evaluate(&format!(
                "Boolean(document.querySelector({}))",
                serde_json::to_string(selector)?
            ))?;
            if found == Value::Bool(true) {
                return Ok(json!({"found": true, "selector": selector}));
            }
            if started.elapsed() >= timeout {
                return Err(Error::Timeout(format!("selector {selector}")));
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }

    pub fn screenshot(&mut self, path: &Path, full_page: bool) -> Result<Value> {
        let params = if full_page {
            let metrics = self.command("Page.getLayoutMetrics", json!({}))?;
            let size = metrics.get("cssContentSize").cloned().unwrap_or(json!({}));
            json!({"format":"png","captureBeyondViewport":true,"clip":{"x":0,"y":0,"width":size.get("width").and_then(Value::as_f64).unwrap_or(1280.0),"height":size.get("height").and_then(Value::as_f64).unwrap_or(720.0),"scale":1}})
        } else {
            json!({"format":"png"})
        };
        let result = self.command("Page.captureScreenshot", params)?;
        let data = result
            .get("data")
            .and_then(Value::as_str)
            .ok_or_else(|| Error::Cdp("screenshot response missing data".into()))?;
        let bytes = STANDARD
            .decode(data)
            .map_err(|e| Error::Cdp(format!("invalid screenshot data: {e}")))?;
        fs::write(path, &bytes)?;
        Ok(json!({"path": path, "bytes": bytes.len(), "format": "png"}))
    }

    pub fn pdf(&mut self, path: &Path, print_background: bool) -> Result<Value> {
        let result = self.command(
            "Page.printToPDF",
            json!({"printBackground": print_background, "preferCSSPageSize": true}),
        )?;
        let data = result
            .get("data")
            .and_then(Value::as_str)
            .ok_or_else(|| Error::Cdp("PDF response missing data".into()))?;
        let bytes = STANDARD
            .decode(data)
            .map_err(|e| Error::Cdp(format!("invalid PDF data: {e}")))?;
        fs::write(path, &bytes)?;
        Ok(json!({"path": path, "bytes": bytes.len(), "format": "pdf"}))
    }

    pub fn snapshot(&mut self) -> Result<Value> {
        self.evaluate("(()=>({url:location.href,title:document.title,text:document.body?.innerText||'',html:document.documentElement?.outerHTML||'',interactive:[...document.querySelectorAll('a,button,input,select,textarea,[role=button]')].slice(0,200).map((e,i)=>({index:i,tag:e.tagName.toLowerCase(),text:(e.innerText||e.value||e.getAttribute('aria-label')||'').trim().slice(0,240),id:e.id||null,name:e.getAttribute('name'),role:e.getAttribute('role')}))}))()")
    }
}
