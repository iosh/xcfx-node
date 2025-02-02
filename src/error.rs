use napi::Error as NapiError;
use std::fmt;

#[derive(Debug)]
pub enum NodeError {
  InitializationError(String),
  ConfigurationError(String),
  RuntimeError(String),
  ShutdownError(String),
}

impl fmt::Display for NodeError {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    match self {
      NodeError::InitializationError(msg) => write!(f, "Initialization Error: {}", msg),
      NodeError::ConfigurationError(msg) => write!(f, "Configuration Error: {}", msg),
      NodeError::RuntimeError(msg) => write!(f, "Runtime Error: {}", msg),
      NodeError::ShutdownError(msg) => write!(f, "Shutdown Error: {}", msg),
    }
  }
}

impl std::error::Error for NodeError {}

impl From<NodeError> for NapiError {
  fn from(error: NodeError) -> Self {
    match error {
      NodeError::InitializationError(msg) => {
        NapiError::from_reason(format!("Initialization Error: {}", msg))
      }
      NodeError::ConfigurationError(msg) => {
        NapiError::from_reason(format!("Configuration Error: {}", msg))
      }
      NodeError::RuntimeError(msg) => NapiError::from_reason(format!("Runtime Error: {}", msg)),
      NodeError::ShutdownError(msg) => NapiError::from_reason(format!("Shutdown Error: {}", msg)),
    }
  }
}

pub type Result<T> = std::result::Result<T, NodeError>;
