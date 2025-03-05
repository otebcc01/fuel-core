use crate::schema::tx::types::TransactionStatus as ApiTxStatus;
use fuel_core_storage::Result as StorageResult;
use fuel_core_txpool::TxStatusMessage;
use fuel_core_types::{
    fuel_types::Bytes32,
    services::txpool::TransactionStatus,
};
use futures::{
    stream::BoxStream,
    Stream,
    StreamExt,
};

#[cfg(test)]
mod test;

#[cfg_attr(test, mockall::automock)]
pub(crate) trait TxnStatusChangeState {
    /// Return the transaction status from the tx pool and database.
    async fn get_tx_status(
        &self,
        id: Bytes32,
    ) -> StorageResult<Option<TransactionStatus>>;
}

#[tracing::instrument(skip(state, stream), fields(transaction_id = %transaction_id))]
pub(crate) async fn transaction_status_change<'a, State>(
    state: State,
    stream: BoxStream<'a, TxStatusMessage>,
    transaction_id: Bytes32,
) -> impl Stream<Item = anyhow::Result<ApiTxStatus>> + 'a
where
    State: TxnStatusChangeState + Send + Sync + 'a,
{
    // Check the database first to see if the transaction already
    // has a status.
    let maybe_db_status = state
        .get_tx_status(transaction_id)
        .await
        .transpose()
        .map(TxStatusMessage::from);

    // Oneshot channel to signal that the stream should be closed.
    let (close, closed) = tokio::sync::oneshot::channel();
    let mut close = Some(close);

    // Chain the initial database check with the stream.
    // Note the option will make an empty stream if it is None.
    futures::stream::iter(maybe_db_status)
        .chain(stream)
        // Keep taking the stream until the oneshot channel is closed.
        .take_until(closed)
        .map(move |status| {
            // Close the stream if the transaction is anything other than
            // `Submitted`.
            if !matches!(
                status,
                TxStatusMessage::Status(TransactionStatus::Submitted { .. })
            ) {
                if let Some(close) = close.take() {
                    let _ = close.send(());
                }
            }

            match status {
                TxStatusMessage::Status(status) => {
                    let status = ApiTxStatus::new(transaction_id, status);
                    Ok(status)
                },
                // Map a failed status to an error for the api.
                TxStatusMessage::FailedStatus => {
                    Err(anyhow::anyhow!("Failed to get transaction status"))
                }
            }
        })
}
