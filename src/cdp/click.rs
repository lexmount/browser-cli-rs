use serde_json::{Value, json};

use super::{Cdp, PointerEvent, PointerViewport, javascript_exception_message};
use crate::{Error, Result};

// Keep the original DOM object across the hover check. Re-querying the selector
// could silently click a replacement element installed by a mousemove handler.
const CLICK_POINT: &str = r#"function(point, observe) {
    const e = this;
    if (!e.isConnected || e.ownerDocument !== document)
        throw new Error('click target is detached');
    if (e.matches(':disabled') || e.closest(
        'button:disabled, input:disabled, select:disabled, textarea:disabled, option:disabled, optgroup:disabled, [inert], [aria-disabled="true" i]'))
        throw new Error('click target is disabled or inert');
    const style = getComputedStyle(e);
    if (style.display === 'none' || style.visibility === 'hidden' || style.visibility === 'collapse')
        throw new Error('click target is not visible');
    if (!point) e.scrollIntoView({block: 'center', inline: 'center', behavior: 'instant'});
    const width = document.documentElement.clientWidth;
    const height = document.documentElement.clientHeight;
    const rects = [...e.getClientRects()].map(r => ({
        left: Math.max(0, r.left), right: Math.min(width, r.right),
        top: Math.max(0, r.top), bottom: Math.min(height, r.bottom)
    })).filter(r => r.right > r.left && r.bottom > r.top);
    if (!rects.length) throw new Error('click target has no visible area in the viewport');
    if (point && !rects.some(r => point.x >= r.left && point.x < r.right &&
                               point.y >= r.top && point.y < r.bottom))
        throw new Error('click target moved after hover; inspect the page and try again');
    const candidates = point ? [point] : rects.map(r => ({
        x: (r.left + r.right) / 2, y: (r.top + r.bottom) / 2
    }));
    // Observation is optional. A page may override these browser properties
    // with throwing getters; that must never turn a valid click into failure.
    const optionalPositive = read => {
        try {
            const value = read();
            return typeof value === 'number' && Number.isFinite(value) && value > 0 ? value : null;
        } catch (_) { return null; }
    };
    const optionalGeneration = () => {
        try {
            const value = performance.timeOrigin;
            return typeof value === 'number' && Number.isFinite(value) && value > 0 ? String(value) : null;
        } catch (_) { return null; }
    };
    for (const candidate of candidates) {
        const hit = document.elementFromPoint(candidate.x, candidate.y);
        if (hit && (hit === e || e.contains(hit))) {
            if (!observe) return candidate;
            return {
                ...candidate,
                viewport_width: optionalPositive(() => window.innerWidth),
                viewport_height: optionalPositive(() => window.innerHeight),
                device_scale: optionalPositive(() => window.devicePixelRatio),
                page_scale: optionalPositive(() => window.visualViewport?.scale),
                page_generation: optionalGeneration()
            };
        }
    }
    throw new Error('click target is covered or cannot receive pointer events');
}"#;

#[derive(Clone, Copy, PartialEq)]
struct Point {
    x: f64,
    y: f64,
}

impl Cdp {
    /// Click a main-document CSS selector using native CDP mouse input.
    ///
    /// Success means the input was dispatched, not that the site's task or
    /// navigation succeeded. No JavaScript-click fallback or tab switching occurs.
    pub fn click(&mut self, selector: &str) -> Result<Value> {
        let response = self.command(
            "Runtime.evaluate",
            json!({
                "expression": format!(
                    "(()=>{{const e=document.querySelector({});if(!e)throw new Error('selector not found');return e}})()",
                    serde_json::to_string(selector)?
                ),
                "returnByValue": false
            }),
        )?;
        let remote = runtime_result(&response)?;
        let object_id = remote["objectId"]
            .as_str()
            .filter(|id| !id.is_empty())
            .ok_or_else(|| Error::Cdp("click target response missing objectId".into()))?;

        let result = self.click_object(object_id);
        // Navigation may already have destroyed the context. Cleanup must not
        // turn a dispatched click into failure or hide its original error.
        let _ = self.command("Runtime.releaseObject", json!({"objectId": object_id}));
        result
    }

    fn click_object(&mut self, object_id: &str) -> Result<Value> {
        let point = self.click_point(object_id, None)?;
        self.mouse_event("mouseMoved", point, "none", 0, 0)?;
        // Hover can move, cover, disable or replace the target. Never chase a
        // changed selector or blindly press at its stale coordinates.
        if self.click_point(object_id, Some(point))? != point {
            return Err(Error::Cdp("click target changed after hover".into()));
        }
        let pressed = self.mouse_event("mousePressed", point, "left", 1, 1);
        // Even if pressing returns an error, best-effort release avoids leaving
        // the button down after a partially handled command. Never repeat press.
        let released = self.mouse_event("mouseReleased", point, "left", 0, 1);
        pressed?;
        released?;
        Ok(json!(true))
    }

    fn click_point(&mut self, object_id: &str, point: Option<Point>) -> Result<Point> {
        let response = self.command(
            "Runtime.callFunctionOn",
            json!({
                "objectId": object_id,
                "functionDeclaration": CLICK_POINT,
                "arguments": [
                    {"value": point.map(|p| json!({"x":p.x,"y":p.y}))},
                    {"value": self.pointer_observer.is_some()}
                ],
                "returnByValue": true
            }),
        )?;
        let value = &runtime_result(&response)?["value"];
        let coordinate = |name| {
            value[name]
                .as_f64()
                .filter(|v| v.is_finite() && *v >= 0.0)
                .ok_or_else(|| Error::Cdp("click target response has invalid coordinates".into()))
        };
        let point = Point {
            x: coordinate("x")?,
            y: coordinate("y")?,
        };
        let positive = |name| value[name].as_f64().filter(|v| v.is_finite() && *v > 0.0);
        self.click_viewport = PointerViewport {
            viewport_width: positive("viewport_width"),
            viewport_height: positive("viewport_height"),
            device_scale: positive("device_scale"),
            page_scale: positive("page_scale"),
            page_generation: value["page_generation"]
                .as_str()
                .filter(|s| {
                    !s.is_empty()
                        && s.len() <= 128
                        && s.bytes().all(|c| c.is_ascii_digit() || c == b'.')
                })
                .map(str::to_owned),
        };
        Ok(point)
    }

    fn mouse_event(
        &mut self,
        event: &str,
        point: Point,
        button: &'static str,
        buttons: u8,
        count: u8,
    ) -> Result<Value> {
        let session_id = self.target_session_id.clone();
        self.send_observed(
            "Input.dispatchMouseEvent",
            json!({"type":event, "x":point.x, "y":point.y,
                   "button":button, "buttons":buttons, "clickCount":count,
                   "pointerType":"mouse", "modifiers":0}),
            Some(&session_id),
            Some(PointerEvent {
                phase: match event {
                    "mouseMoved" => "move",
                    "mousePressed" => "down",
                    _ => "up",
                },
                x: point.x,
                y: point.y,
                coordinate_space: "css_viewport",
                button,
                buttons,
                click_count: count,
                viewport: self.click_viewport.clone(),
            }),
        )
    }
}

fn runtime_result(response: &Value) -> Result<&Value> {
    if let Some(exception) = response.get("exceptionDetails") {
        return Err(Error::Cdp(javascript_exception_message(exception)));
    }
    Ok(&response["result"])
}
