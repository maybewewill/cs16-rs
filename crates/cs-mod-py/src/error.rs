//! Error types and Python-formatted tracebacks for `cs-mod-py`.

use std::fmt;

use cs_mod_host::RuntimeError;

/// One stack frame recorded during Python execution for `ModFaultDump` diagnostics.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CallFrame {
    pub file: String,
    pub func_name: String,
    pub line: usize,
}

/// Errors produced while lexing, parsing, or executing a Python plugin.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PyError {
    Syntax {
        file: String,
        line: usize,
        col: usize,
        message: String,
    },
    OutOfFuel {
        budget: u64,
        frames: Vec<CallFrame>,
    },
    MemoryLimit {
        limit: usize,
        frames: Vec<CallFrame>,
    },
    Runtime {
        kind: String,
        message: String,
        frames: Vec<CallFrame>,
    },
}

impl PyError {
    pub fn syntax(file: impl Into<String>, line: usize, col: usize, msg: impl Into<String>) -> Self {
        Self::Syntax {
            file: file.into(),
            line,
            col,
            message: msg.into(),
        }
    }

    pub fn runtime(
        kind: impl Into<String>,
        message: impl Into<String>,
        frames: Vec<CallFrame>,
    ) -> Self {
        Self::Runtime {
            kind: kind.into(),
            message: message.into(),
            frames,
        }
    }

    pub fn is_out_of_fuel(&self) -> bool {
        matches!(self, Self::OutOfFuel { .. })
    }

    pub fn exception_kind(&self) -> &str {
        match self {
            Self::Syntax { .. } => "SyntaxError",
            Self::OutOfFuel { .. } => "OutOfFuel",
            Self::MemoryLimit { .. } => "MemoryError",
            Self::Runtime { kind, .. } => kind.as_str(),
        }
    }

    pub fn message_text(&self) -> String {
        match self {
            Self::Syntax { message, .. } => message.clone(),
            Self::OutOfFuel { budget, .. } => {
                format!("exceeded per-call instruction fuel budget ({budget})")
            }
            Self::MemoryLimit { limit, .. } => {
                format!("exceeded plugin memory limit ({limit} bytes)")
            }
            Self::Runtime { message, .. } => message.clone(),
        }
    }

    pub fn frames(&self) -> &[CallFrame] {
        match self {
            Self::Syntax { .. } => &[],
            Self::OutOfFuel { frames, .. }
            | Self::MemoryLimit { frames, .. }
            | Self::Runtime { frames, .. } => frames,
        }
    }

    /// Formats a standard Python traceback suitable for logs and `ModFaultDump`.
    pub fn format_traceback(&self) -> String {
        match self {
            Self::Syntax {
                file,
                line,
                col,
                message,
            } => format!("  File \"{file}\", line {line}, column {col}\nSyntaxError: {message}"),
            Self::OutOfFuel { budget, frames } => {
                let mut out = String::from("Traceback (most recent call last):\n");
                for f in frames {
                    out.push_str(&format!(
                        "  File \"{}\", line {}, in {}\n",
                        f.file, f.line, f.func_name
                    ));
                }
                out.push_str(&format!(
                    "OutOfFuel: exceeded per-call instruction fuel budget ({budget})"
                ));
                out
            }
            Self::MemoryLimit { limit, frames } => {
                let mut out = String::from("Traceback (most recent call last):\n");
                for f in frames {
                    out.push_str(&format!(
                        "  File \"{}\", line {}, in {}\n",
                        f.file, f.line, f.func_name
                    ));
                }
                out.push_str(&format!(
                    "MemoryError: exceeded plugin memory limit ({limit} bytes)"
                ));
                out
            }
            Self::Runtime {
                kind,
                message,
                frames,
            } => {
                let mut out = String::from("Traceback (most recent call last):\n");
                for f in frames {
                    out.push_str(&format!(
                        "  File \"{}\", line {}, in {}\n",
                        f.file, f.line, f.func_name
                    ));
                }
                if message.is_empty() {
                    out.push_str(kind);
                } else {
                    out.push_str(&format!("{kind}: {message}"));
                }
                out
            }
        }
    }

    pub fn into_host_error(self, mod_id: &str) -> RuntimeError {
        match self {
            Self::Syntax { .. } => {
                RuntimeError::Compile(mod_id.to_string(), self.format_traceback())
            }
            other => RuntimeError::Fault(other.format_traceback()),
        }
    }
}

impl fmt::Display for PyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.format_traceback())
    }
}

impl std::error::Error for PyError {}
