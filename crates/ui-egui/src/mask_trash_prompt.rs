//! A layer mask dragged onto the Layers panel's Trash asks first, as in Photoshop (#3113): "Apply
//! mask to layer before removing?" Apply runs `layer.layerMask.apply`, Delete
//! `layer.layerMask.delete`, Cancel keeps the mask. A click on the Trash still deletes the
//! targeted mask at once (#2197).
//!
//! The prompt is an ordinary dialog in the UI state: `ui.inspect` shows it, `ui.dialog.confirm`
//! applies, `ui.dialog.set` with `"choice": "delete"` then `ui.dialog.confirm` deletes, and
//! `ui.dialog.cancel` keeps the mask.

use photocraft_doc::LayerId;
use serde_json::{Map, Value, json};

use crate::PhotocraftApp;
use crate::state::DialogKind;

const MARK: &str = "__maskTrash";

pub fn owns(fields: &Map<String, Value>) -> bool {
    fields.contains_key(MARK)
}

/// The action a mask thumbnail dropped on the Trash pushes; [`intercept`] turns it into the prompt.
pub fn drop_action(layer: u64) -> (String, Value) {
    ("layer.layerMask.delete".into(), json!({"layer": layer, MARK: true}))
}

/// Opens the prompt for a [`drop_action`]; `None` for every other command.
pub fn intercept(app: &mut PhotocraftApp, command: &str, params: &Value) -> Option<Result<Value, String>> {
    if command != "layer.layerMask.delete" || params.get(MARK).and_then(Value::as_bool) != Some(true) {
        return None;
    }
    let layer = params.get("layer").and_then(Value::as_u64)?;
    let state = app.session.active()?;
    state.doc.layer(LayerId(layer)).filter(|l| l.mask.is_some())?;
    // Apply needs a pixel layer (Layer › Layer Mask › Apply is greyed out otherwise).
    let can_apply = app.session.is_enabled_with("layer.layerMask.apply", &json!({"layer": layer}));
    let fields = json!({MARK: true, "__label": "Delete Layer Mask", "message": "Apply mask to layer before removing?",
        "document": state.doc.id.0, "layer": layer, "canApply": can_apply, "choice": if can_apply { "apply" } else { "delete" }});
    if app.ui.dialogs.iter().any(|d| owns(&d.fields)) {
        return Some(Ok(json!({"confirmationPending": true})));
    }
    let id = app.ui.open_dialog(DialogKind::Command, fields.as_object().cloned().unwrap_or_default());
    Some(Ok(json!({"dialog": id, "confirmationPending": true})))
}

pub fn body(ui: &mut egui::Ui, fields: &Map<String, Value>) {
    ui.label(tl!(fields.get("message").and_then(Value::as_str).unwrap_or("Apply mask to layer before removing?")));
}

/// Apply is the default button; it's greyed out where the mask can't be applied.
pub fn can_apply(fields: &Map<String, Value>) -> bool {
    fields.get("canApply").and_then(Value::as_bool).unwrap_or(false)
}

/// Runs the answer: `choice` "delete" deletes the mask, anything else applies it.
pub fn confirm(app: &mut PhotocraftApp, fields: &Map<String, Value>) -> Result<Value, String> {
    let state = app.session.active().ok_or(tl!("The document is closed"))?;
    let layer = fields.get("layer").and_then(Value::as_u64);
    let same = fields.get("document").and_then(Value::as_u64) == Some(state.doc.id.0);
    let Some(layer) = layer.filter(|id| same && state.doc.layer(LayerId(*id)).is_some_and(|l| l.mask.is_some())) else {
        return Err(tl!("The layer or its mask changed. Drag the mask onto the Trash again.").into());
    };
    let command = if fields.get("choice").and_then(Value::as_str) == Some("delete") { "layer.layerMask.delete" } else { "layer.layerMask.apply" };
    app.run_command(command, json!({"layer": layer}))
}
