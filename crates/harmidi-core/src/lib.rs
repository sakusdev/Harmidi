mod analysis;
mod dsp;
mod tracking;

use analysis::{AnalysisOptions, AnalysisResult};
use js_sys::Float32Array;
use wasm_bindgen::prelude::*;

#[wasm_bindgen]
pub fn analyze_pcm(
    samples: &Float32Array,
    sample_rate: u32,
    options: JsValue,
) -> Result<JsValue, JsValue> {
    if sample_rate == 0 {
        return Err(JsValue::from_str("sample_rate must be greater than zero"));
    }
    if samples.length() == 0 {
        return Err(JsValue::from_str("audio buffer is empty"));
    }

    let options: AnalysisOptions = serde_wasm_bindgen::from_value(options)
        .map_err(|error| JsValue::from_str(&format!("invalid analysis options: {error}")))?;

    let mut pcm = vec![0.0_f32; samples.length() as usize];
    samples.copy_to(&mut pcm);

    let result: AnalysisResult = analysis::analyze(&pcm, sample_rate, &options)
        .map_err(|error| JsValue::from_str(&error))?;

    serde_wasm_bindgen::to_value(&result)
        .map_err(|error| JsValue::from_str(&format!("failed to serialize result: {error}")))
}
