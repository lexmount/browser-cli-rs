use serde_json::{Value, json};
use std::{
    thread,
    time::{Duration, Instant},
};

use super::{Cdp, PointerEvent, PointerOrigin, PointerViewport, javascript_exception_message};
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
                target_rects: rects,
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

#[derive(Clone, Copy, Debug, PartialEq)]
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
        self.click_impl(selector, false)
    }

    /// Opt-in coordinates of this one real click; the ordinary click result stays `true`.
    pub fn click_with_point(&mut self, selector: &str) -> Result<Value> {
        self.click_impl(selector, true)
    }

    fn pointer_element(&mut self, selector: &str) -> Result<String> {
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
        remote["objectId"]
            .as_str()
            .filter(|id| !id.is_empty())
            .map(str::to_owned)
            .ok_or_else(|| Error::Cdp("click target response missing objectId".into()))
    }

    fn click_impl(&mut self, selector: &str, report_point: bool) -> Result<Value> {
        let object_id = self.pointer_element(selector)?;
        let result = self.click_object(&object_id, report_point);
        // Navigation may already have destroyed the context. Cleanup must not
        // turn a dispatched click into failure or hide its original error.
        let _ = self.command("Runtime.releaseObject", json!({"objectId": object_id}));
        result
    }

    fn click_object(&mut self, object_id: &str, report_point: bool) -> Result<Value> {
        let observe = report_point || self.pointer_observer.is_some();
        let point = self.click_point(object_id, None, observe)?;
        self.mouse_event("mouseMoved", point, "none", 0, 0)?;
        // Hover can move, cover, disable or replace the target. Never chase a
        // changed selector or blindly press at its stale coordinates.
        if self.click_point(object_id, Some(point), observe)? != point {
            return Err(Error::Cdp("click target changed after hover".into()));
        }
        let pressed = self.mouse_event("mousePressed", point, "left", 1, 1);
        // Even if pressing returns an error, best-effort release avoids leaving
        // the button down after a partially handled command. Never repeat press.
        let released = self.mouse_event("mouseReleased", point, "left", 0, 1);
        pressed?;
        released?;
        Ok(if report_point {
            self.point_result(point)
        } else {
            json!(true)
        })
    }

    /// Move through real CDP samples from the caller's last known input point.
    /// No press, release, JavaScript click, alternate Target or invented origin.
    pub fn move_pointer(
        &mut self,
        selector: &str,
        origin: &PointerOrigin,
        duration: Duration,
    ) -> Result<Value> {
        if !origin.x.is_finite()
            || !origin.y.is_finite()
            || origin.target_id != self.target_id
            || origin.coordinate_space != "css_viewport"
            || duration < Duration::from_millis(100)
            || duration > Duration::from_millis(2000)
        {
            return Err(Error::Config(
                "move requires finite coordinates and duration 100...2000 ms".into(),
            ));
        }
        let object_id = self.pointer_element(selector)?;
        let result = self.move_object(
            &object_id,
            Point {
                x: origin.x,
                y: origin.y,
            },
            duration,
            &origin.viewport,
        );
        let _ = self.command("Runtime.releaseObject", json!({"objectId":object_id}));
        result
    }

    fn move_object(
        &mut self,
        object_id: &str,
        from: Point,
        duration: Duration,
        origin_viewport: &PointerViewport,
    ) -> Result<Value> {
        let destination = self.click_point(object_id, None, true)?;
        let viewport = self.click_viewport.clone();
        let rects = self.click_target_rects.clone();
        let width = viewport.viewport_width;
        let height = viewport.viewport_height;
        if width.is_none()
            || height.is_none()
            || viewport.device_scale.is_none()
            || viewport.page_scale.is_none()
            || viewport.page_generation.is_none()
            || rects.is_none()
        {
            return Err(Error::Cdp(
                "move requires known document and viewport metadata".into(),
            ));
        }
        if from.x < 0.0 || from.y < 0.0 || from.x >= width.unwrap() || from.y >= height.unwrap() {
            return Err(Error::Config(
                "move origin must be inside the current CSS viewport".into(),
            ));
        }
        if origin_viewport != &viewport {
            return Err(Error::Cdp(
                "move origin document or viewport no longer matches".into(),
            ));
        }
        let started = Instant::now();
        self.confirmed_move(from)?;
        let mut steps = 1_u64;
        loop {
            if let Some(remaining) = duration.checked_sub(started.elapsed()) {
                thread::sleep(remaining.min(Duration::from_micros(16_667)));
            }
            // Keep the original DOM object and endpoint. Hover-induced layout,
            // navigation, overlays or viewport changes stop, never chase a new point.
            if self.click_point(object_id, Some(destination), true)? != destination {
                return Err(Error::Cdp("move target point changed during hover".into()));
            }
            if self.click_viewport != viewport {
                return Err(Error::Cdp(
                    "move document or viewport changed during hover".into(),
                ));
            }
            if self.click_target_rects != rects {
                return Err(Error::Cdp(
                    "move target geometry changed during hover".into(),
                ));
            }
            let progress = (started.elapsed().as_secs_f64() / duration.as_secs_f64()).min(1.0);
            self.confirmed_move(interpolate(from, destination, progress))?;
            steps += 1;
            if progress >= 1.0 {
                break;
            }
        }
        self.click_point(object_id, Some(destination), true)?;
        if self.click_viewport != viewport || self.click_target_rects != rects {
            return Err(Error::Cdp(
                "move document or viewport changed after hover".into(),
            ));
        }
        let mut result = self.point_result(destination);
        result["steps"] = json!(steps);
        result["elapsed_ms"] = json!(started.elapsed().as_millis());
        result["requested_duration_ms"] = json!(duration.as_millis());
        Ok(result)
    }

    fn point_result(&self, point: Point) -> Value {
        let mut value = serde_json::to_value(&self.click_viewport).unwrap_or_else(|_| json!({}));
        value["x"] = json!(point.x);
        value["y"] = json!(point.y);
        value["coordinate_space"] = json!("css_viewport");
        value["target_id"] = json!(self.target_id);
        value
    }

    fn confirmed_move(&mut self, point: Point) -> Result<Value> {
        self.mouse_event("mouseMoved", point, "none", 0, 0)
    }

    fn click_point(
        &mut self,
        object_id: &str,
        point: Option<Point>,
        observe: bool,
    ) -> Result<Point> {
        let response = self.command(
            "Runtime.callFunctionOn",
            json!({
                "objectId": object_id,
                "functionDeclaration": CLICK_POINT,
                "arguments": [
                    {"value": point.map(|p| json!({"x":p.x,"y":p.y}))},
                    {"value": observe}
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
        self.click_target_rects = value
            .get("target_rects")
            .filter(|rects| {
                rects.as_array().is_some_and(|rects| {
                    !rects.is_empty()
                        && rects.iter().all(|rect| {
                            ["left", "right", "top", "bottom"]
                                .iter()
                                .all(|key| rect[*key].as_f64().is_some_and(f64::is_finite))
                        })
                })
            })
            .cloned();
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

fn interpolate(from: Point, to: Point, progress: f64) -> Point {
    let eased = progress * progress * (3.0 - 2.0 * progress);
    Point {
        x: (1.0 - eased) * from.x + eased * to.x,
        y: (1.0 - eased) * from.y + eased * to.y,
    }
}

fn runtime_result(response: &Value) -> Result<&Value> {
    if let Some(exception) = response.get("exceptionDetails") {
        return Err(Error::Cdp(javascript_exception_message(exception)));
    }
    Ok(&response["result"])
}
