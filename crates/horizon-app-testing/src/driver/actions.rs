use serde_json::{Value, json};

use super::NativeDriver;
use crate::contract::Platform;
use crate::recipe::{Action, Direction, Orientation, Point, Target};
use crate::{Error, Result};

pub(super) fn dispatch(driver: &mut NativeDriver, action: &Action) -> Result<()> {
    match action {
        Action::Tap { target } => if let Target::Coordinates(point) = target {
            gesture(driver, *point, *point, 0)
        } else {
                let element = driver.element(target)?;
                driver.request("POST", &format!("/element/{element}/click"), Some(&json!({}))).map(|_| ())
        },
        Action::LongPress { target, duration_millis } => {
            let origin = match target {
                Target::Coordinates(point) => json!({"origin":"viewport", "x":point.x,"y":point.y}),
                _ => json!({"origin":{"element-6066-11e4-a52e-4f735466cecf":driver.element(target)?},"x":0,"y":0}),
            };
            let mut move_action = origin;
            move_action["type"] = json!("pointerMove");
            move_action["duration"] = json!(0);
            pointers(driver, &[move_action,json!({"type":"pointerDown","button":0}),json!({"type":"pause","duration":duration_millis}),json!({"type":"pointerUp","button":0})])
        }
        Action::Type { target, text } => {
            let element = driver.element(target)?;
            driver.request("POST", &format!("/element/{element}/value"), Some(&json!({"text":text,"value":text.chars().map(|c| c.to_string()).collect::<Vec<_>>()}))).map(|_| ())
        }
        Action::Clear { target } => {
            let element = driver.element(target)?;
            driver.request("POST", &format!("/element/{element}/clear"), Some(&json!({}))).map(|_| ())
        }
        Action::Swipe { from, to, duration_millis } => gesture(driver,*from,*to,*duration_millis),
        Action::Scroll { direction, distance } => {
            let rect = driver.request("GET", "/window/rect", None)?;
            let width = rect.pointer("/value/width").and_then(Value::as_u64).filter(|n| *n <= 16_384).ok_or(Error::DriverInvalid)?;
            let height = rect.pointer("/value/height").and_then(Value::as_u64).filter(|n| *n <= 16_384).ok_or(Error::DriverInvalid)?;
            let width = u32::try_from(width).map_err(|_| Error::DriverInvalid)?;
            let height = u32::try_from(height).map_err(|_| Error::DriverInvalid)?;
            if width < 4 || height < 4 { return Err(Error::DriverInvalid); }
            let (horizontal, positive) = match direction { Direction::Up => (false,true),Direction::Down => (false,false),Direction::Left => (true,true),Direction::Right => (true,false) };
            let limit = if horizontal { width } else { height };
            let distance = (*distance).min(limit / 2);
            let center = Point { x:width / 2,y:height / 2 };
            let mut from = center; let mut to = center;
            let (start,end) = if positive { (limit/2-distance/2,limit/2+distance/2) } else { (limit/2+distance/2,limit/2-distance/2) };
            if horizontal { from.x=start;to.x=end; } else { from.y=start;to.y=end; }
            gesture(driver,from,to,600)
        }
        Action::Back {} => driver.request("POST", "/back", Some(&json!({}))).map(|_| ()),
        Action::Home {} => if driver.platform == Platform::Ios {
            driver.execute("mobile: pressButton", &json!({"name":"home"})).map(|_| ())
        } else {
            driver.execute("mobile: pressKey",&json!({"keycode":3})).map(|_| ())
        },
        Action::Rotate { orientation } => driver.request("POST", "/orientation", Some(&json!({"orientation":match orientation {Orientation::Portrait=>"PORTRAIT",Orientation::Landscape=>"LANDSCAPE"}}))).map(|_| ()),
        Action::Launch {} => {
            if driver.platform == Platform::Ios {
                driver.execute("mobile: launchApp", &json!({"bundleId":driver.app_id,"environment":driver.arguments})).map(|_| ())
            } else {
                let extras: Vec<_> = driver.arguments.iter().map(|(key,value)|json!(["s",key,value])).collect();
                driver.execute("mobile: startActivity", &json!({"action":"android.intent.action.MAIN","package":driver.app_id,"categories":["android.intent.category.LAUNCHER"],"extras":extras,"wait":true})).map(|_| ())
            }
        },
        Action::Terminate {} => driver.execute("mobile: terminateApp",&app_argument(driver)).map(|_| ()),
        Action::Reset {} => Err(Error::ResetRequiresReallocation),
        Action::DeepLink { url } => {
            let mut args = if driver.platform == Platform::Ios { app_argument(driver) } else { json!({"package":driver.app_id}) }; args["url"] = json!(url);
            driver.execute("mobile: deepLink",&args).map(|_| ())
        }
        _ => Err(Error::RecipeInvalid),
    }
}

fn app_argument(driver: &NativeDriver) -> Value {
    match driver.platform {
        Platform::Ios => json!({"bundleId":driver.app_id}),
        Platform::Android => json!({"appId":driver.app_id}),
    }
}

fn gesture(driver: &NativeDriver, from: Point, to: Point, duration: u64) -> Result<()> {
    pointers(
        driver,
        &[
            json!({"type":"pointerMove","duration":0,"origin":"viewport","x":from.x,"y":from.y}),
            json!({"type":"pointerDown","button":0}),
            json!({"type":"pointerMove","duration":duration,"origin":"viewport","x":to.x,"y":to.y}),
            json!({"type":"pointerUp","button":0}),
        ],
    )
}

fn pointers(driver: &NativeDriver, actions: &[Value]) -> Result<()> {
    let outcome = driver.request("POST", "/actions", Some(&json!({"actions":[{"type":"pointer","id":"native-touch","parameters":{"pointerType":"touch"},"actions":actions}]})));
    let released = driver.request("DELETE", "/actions", None);
    outcome.and(released).map(|_| ())
}
