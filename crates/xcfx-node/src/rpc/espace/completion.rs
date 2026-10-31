//! Completes accepted sends with receipts visible to standard eSpace queries.

use std::time::Duration;

use cfx_types::H256;
use serde_json::json;
use tokio::time::timeout;

use crate::{
  rpc::{
    operation::{CompletionContext, ResponseCompletion, RpcResult},
    protocol::RpcError,
  },
  runtime::RuntimeTransactionError,
};

use super::{convert, error};

const MAX_RECEIPT_WAIT: Duration = Duration::from_secs(30);

pub(super) fn transaction_receipt(
  result: Result<H256, RuntimeTransactionError>,
  timeout_ms: Option<u64>,
) -> ResponseCompletion {
  let hash = match result {
    Ok(hash) => hash,
    Err(error) => return ResponseCompletion::Ready(Err(error::transaction_error(error))),
  };
  let duration = timeout_ms
    .filter(|milliseconds| *milliseconds > 0)
    .map(Duration::from_millis)
    .unwrap_or(MAX_RECEIPT_WAIT)
    .min(MAX_RECEIPT_WAIT);

  // This wait starts after the source batch and its automatic mining finish.
  // One deadline covers both queued receipt reads and change notifications.
  ResponseCompletion::wait(move |context| async move {
    match timeout(duration, wait_for_receipt(context, hash)).await {
      Ok(result) => result,
      Err(_) => Err(RpcError {
        code: 4,
        message: "Transaction confirmation timeout".into(),
        data: Some(json!(hash)),
      }),
    }
  })
}

async fn wait_for_receipt(mut context: CompletionContext, hash: H256) -> RpcResult {
  loop {
    let receipt = context
      .node()
      .call(move |node| node.espace_transaction_receipt_by_hash(&hash))
      .await
      .map_err(|error| RpcError::from(&error))?;

    match receipt {
      // The context subscribed before this query. A concurrent commit therefore
      // leaves an unread wakeup; each subsequent read uses the current Runtime.
      Ok(None) => context.changed().await,
      result => return convert::receipt(result),
    }
  }
}
