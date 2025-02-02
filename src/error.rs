use napi::Error as NapiError;
use std::fmt;

#[derive(Debug)]
pub enum NodeError {
  Initialization(String),
  Configuration(String),
  Runtime(String),
  Shutdown(String),
}

impl fmt::Display for NodeError {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    match self {
      NodeError::Initialization(msg) => write!(f, "Initialization Error: {}", msg),
      NodeError::Configuration(msg) => write!(f, "Configuration Error: {}", msg),
      NodeError::Runtime(msg) => write!(f, "Runtime Error: {}", msg),
      NodeError::Shutdown(msg) => write!(f, "Shutdown Error: {}", msg),
    }
  }
}

impl std::error::Error for NodeError {}

impl From<NodeError> for NapiError {
  fn from(error: NodeError) -> Self {
    match error {
      NodeError::Initialization(msg) => {
        NapiError::from_reason(format!("Initialization Error: {}", msg))
      }
      NodeError::Configuration(msg) => {
        NapiError::from_reason(format!("Configuration Error: {}", msg))
      }
      NodeError::Runtime(msg) => NapiError::from_reason(format!("Runtime Error: {}", msg)),
      NodeError::Shutdown(msg) => NapiError::from_reason(format!("Shutdown Error: {}", msg)),
    }
  }
}

pub type Result<T> = std::result::Result<T, NodeError>;
