//! Runs one writing action against the ChatGPT plan and records it in the learning log.

use crate::actions::{self, Action, Feedback};
use crate::config::{Config, State};
use crate::http::HttpError;
use crate::{api, auth, learn};
use anyhow::{Result, bail};

pub const MAX_INPUT_CHARS: usize = 12_000;

pub struct Outcome {
    pub feedback: Feedback,
    /// False when the model answered with plain text instead of feedback JSON.
    pub structured: bool,
    pub model: String,
}

/// Configured model, else the one picked earlier, else the first model the plan lists.
pub fn resolve_model(cfg: &Config, state: &mut State, token: &str) -> Result<String> {
    if !cfg.model.trim().is_empty() {
        return Ok(cfg.model.trim().to_string());
    }
    if !state.model.is_empty() {
        return Ok(state.model.clone());
    }
    let models = api::models(auth::RESOURCE, token)?;
    let Some(first) = models.first() else {
        bail!("Your ChatGPT plan lists no models. Run `enmaster models` to check.");
    };
    state.model = first.id.clone();
    state.effort_supported = None;
    state.save()?;
    Ok(state.model.clone())
}

pub fn run(cfg: &Config, action: &Action, text: &str, progress: &mut dyn FnMut(usize)) -> Result<Outcome> {
    let text = text.trim();
    if text.is_empty() {
        bail!("There is no text. Select some text first, or type it in the box.");
    }
    if text.chars().count() > MAX_INPUT_CHARS {
        bail!("The text is longer than {MAX_INPUT_CHARS} characters. Select a shorter part.");
    }
    let token = auth::access_token()?;
    let mut state = State::load();
    let model = resolve_model(cfg, &mut state, &token)?;
    let instructions = actions::instructions(cfg, action);

    // Structured outputs and reasoning effort aren't documented for plan tokens yet:
    // try them, and remember if the endpoint rejects them.
    let mut use_schema = state.schema_supported != Some(false);
    let wants_effort = matches!(cfg.reasoning_effort.as_str(), "low" | "medium" | "high");
    let mut use_effort = wants_effort && state.effort_supported != Some(false);
    loop {
        let request = api::Request {
            model: &model,
            instructions: &instructions,
            input: text,
            effort: use_effort.then_some(cfg.reasoning_effort.as_str()),
            schema: use_schema.then(actions::schema),
        };
        match api::respond(auth::RESOURCE, &token, &request, progress) {
            Ok(raw) => {
                let mut changed = false;
                if use_schema && state.schema_supported != Some(true) {
                    state.schema_supported = Some(true);
                    changed = true;
                }
                if use_effort && state.effort_supported != Some(true) {
                    state.effort_supported = Some(true);
                    changed = true;
                }
                if changed {
                    let _ = state.save();
                }
                let (feedback, structured) = actions::parse_feedback(&raw);
                return Ok(Outcome { feedback, structured, model });
            }
            Err(e) => {
                let Some(http) = e.downcast_ref::<HttpError>() else { return Err(e) };
                if use_schema && http.rejects_param("text") {
                    use_schema = false;
                    state.schema_supported = Some(false);
                } else if use_effort && http.rejects_param("reasoning") {
                    use_effort = false;
                    state.effort_supported = Some(false);
                } else {
                    return Err(e);
                }
                let _ = state.save();
            }
        }
    }
}

/// Append a finished result to the learning log (only feedback the model returned as JSON).
pub fn log(cfg: &Config, action: &Action, text: &str, outcome: &Outcome) {
    if !outcome.structured {
        return;
    }
    if let Err(e) = learn::record(cfg, &action.id, &outcome.model, text, &outcome.feedback) {
        eprintln!("enmaster: could not write the learning log: {e:#}");
    }
}

/// Run and append the result to the learning log.
pub fn run_and_log(cfg: &Config, action: &Action, text: &str, progress: &mut dyn FnMut(usize)) -> Result<Outcome> {
    let outcome = run(cfg, action, text, progress)?;
    log(cfg, action, text, &outcome);
    Ok(outcome)
}
