use std::io::Write as _;

use serde::Serialize;

use crate::{
    domain::ArtifactRefV1,
    error::{Error, ErrorBody, Result},
};

#[derive(Debug, Serialize)]
pub struct Envelope<T: Serialize> {
    pub version: u32,
    pub ok: bool,
    pub status: &'static str,
    pub command: String,
    /// Exhaustive command-effect classification from the CLI surface.
    pub effect: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<T>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<ErrorBody<'static>>,
    pub warnings: Vec<String>,
    pub transactions: Vec<serde_json::Value>,
}

#[derive(Debug, Serialize)]
pub struct CommandData {
    pub result: serde_json::Value,
    pub artifact: Option<ArtifactRefV1>,
}

pub fn success(command: &str, effect: &'static str, data: CommandData) -> Result<()> {
    write_envelope(&Envelope {
        version: 1,
        ok: true,
        status: "ok",
        command: command.to_owned(),
        effect,
        data: Some(data),
        error: None,
        warnings: Vec::new(),
        transactions: Vec::new(),
    })
}

pub fn failure(command: &str, effect: &'static str, error: &Error) -> Result<()> {
    write_envelope(&Envelope::<serde_json::Value> {
        version: 1,
        ok: false,
        command: command.to_owned(),
        status: "error",
        effect,
        data: None,
        error: Some(ErrorBody {
            code: error.code(),
            message: error.to_string(),
            context: error.context(),
        }),
        warnings: Vec::new(),
        transactions: Vec::new(),
    })
}

fn write_envelope<T: Serialize>(envelope: &Envelope<T>) -> Result<()> {
    let mut stdout = std::io::stdout().lock();
    serde_json::to_writer(&mut stdout, envelope)?;
    stdout.write_all(b"\n")?;
    Ok(())
}
