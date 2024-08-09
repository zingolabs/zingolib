use std::{cmp::max, sync::Arc};

use crate::config::ZingoConfig;
use crate::grpc_connector::GrpcConnector;
use log::debug;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};
use zcash_client_backend::proto::compact_formats::CompactBlock;
pub struct FetchCompactBlocks {
    config: ZingoConfig,
}

impl FetchCompactBlocks {
    pub fn new(config: &ZingoConfig) -> Self {
        Self {
            config: config.clone(),
        }
    }

    async fn fetch_blocks_range(
        &self,
        senders: &[UnboundedSender<CompactBlock>; 2],
        start_block: u64,
        end_block: u64,
    ) -> Result<(), String> {
        let grpc_client = Arc::new(GrpcConnector::new(self.config.get_lightwalletd_uri()));
        const STEP: u64 = 10_000;

        // We need the `rev()` here because rust ranges can only go up
        for b in (end_block..(start_block + 1)).rev().step_by(STEP as usize) {
            let start = b;
            let end = max((b as i64) - (STEP as i64) + 1, end_block as i64) as u64;
            if start < end {
                return Err("Wrong block order".to_string());
            }

            debug!("Fetching blocks {}-{}", start, end);

            crate::grpc_connector::get_block_range(&grpc_client, start, end, senders).await?;
        }

        Ok(())
    }

    // Load all the blocks from LightwalletD
    pub async fn start(
        &self,
        senders: [UnboundedSender<CompactBlock>; 2],
        start_block: u64,
        end_block: u64,
        mut reorg_receiver: UnboundedReceiver<Option<u64>>,
    ) -> Result<(), String> {
        if start_block < end_block {
            return Err("Expected blocks in reverse order".to_string());
        }

        //debug!("Starting fetch compact blocks");
        self.fetch_blocks_range(&senders, start_block, end_block)
            .await?;

        // After fetching all the normal blocks, we actually wait to see if any re-org'd blocks are received
        while let Some(Some(reorg_block)) = reorg_receiver.recv().await {
            // Fetch the additional block.
            self.fetch_blocks_range(&senders, reorg_block, reorg_block)
                .await?;
        }

        //debug!("Finished fetch compact blocks, closing channels");
        Ok(())
    }
}
