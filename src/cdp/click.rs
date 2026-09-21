use serde_json::{Value, json};

use super::{Cdp, javascript_exception_message};
use crate::{Error, Result};

// Keep the original DOM object across the hover check. Re-querying the selector
// could silently click a replacement element installed by a mousemove handler.
const CLICK_POINT: &str = r#"function(point) {
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
    for (const candidate of candidates) {
        const hit = document.elementFromPoint(candidate.x, candidate.y);
        if (hit && (hit === e || e.contains(hit))) return candidate;
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
                "arguments": [{"value": point.map(|p| json!({"x":p.x,"y":p.y}))}],
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
        Ok(Point {
            x: coordinate("x")?,
            y: coordinate("y")?,
        })
    }

    fn mouse_event(
        &mut self,
        event: &str,
        point: Point,
        button: &str,
        buttons: u8,
        count: u8,
    ) -> Result<Value> {
        self.command(
            "Input.dispatchMouseEvent",
            json!({"type":event, "x":point.x, "y":point.y,
                   "button":button, "buttons":buttons, "clickCount":count,
                   "pointerType":"mouse", "modifiers":0}),
        )
    }
}

fn runtime_result(response: &Value) -> Result<&Value> {
    if let Some(exception) = response.get("exceptionDetails") {
        return Err(Error::Cdp(javascript_exception_message(exception)));
    }
    Ok(&response["result"])
}
