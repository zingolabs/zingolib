use std::{
    collections::HashMap,
    sync::{
        atomic::{self, AtomicBool},
        Arc,
    },
};

use tokio::{sync::mpsc, task::JoinHandle};

use zcash_client_backend::data_api::scanning::ScanRange;
use zcash_keys::keys::UnifiedFullViewingKey;
use zcash_primitives::{consensus::Parameters, zip32::AccountId};

use crate::{client::FetchRequest, primitives::WalletBlock};

use super::{error::ScanError, scan, ScanResults};

const SCAN_WORKER_POOLSIZE: usize = 2;

pub(crate) struct Scanner<P> {
    workers: Vec<WorkerHandle>,
    workers_count: usize,
    scan_results_sender: mpsc::UnboundedSender<(ScanRange, Result<ScanResults, ScanError>)>,
    fetch_request_sender: mpsc::UnboundedSender<FetchRequest>,
    parameters: P,
    ufvks: HashMap<AccountId, UnifiedFullViewingKey>,
}

// TODO: add fn for checking and handling worker errors
impl<P> Scanner<P>
where
    P: Parameters + Sync + Send + 'static,
{
    pub(crate) fn new(
        scan_results_sender: mpsc::UnboundedSender<(ScanRange, Result<ScanResults, ScanError>)>,
        fetch_request_sender: mpsc::UnboundedSender<FetchRequest>,
        parameters: P,
        ufvks: HashMap<AccountId, UnifiedFullViewingKey>,
    ) -> Self {
        let workers: Vec<WorkerHandle> = Vec::with_capacity(SCAN_WORKER_POOLSIZE);

        Self {
            workers,
            workers_count: 0,
            scan_results_sender,
            fetch_request_sender,
            parameters,
            ufvks,
        }
    }

    pub(crate) fn spawn_workers(&mut self) {
        for _ in 0..SCAN_WORKER_POOLSIZE {
            let (scan_task_sender, scan_task_receiver) = mpsc::unbounded_channel();
            let worker = ScanWorker::new(
                scan_task_receiver,
                self.scan_results_sender.clone(),
                self.fetch_request_sender.clone(),
                self.parameters.clone(),
                self.ufvks.clone(),
            );
            let is_scanning = Arc::clone(&worker.is_scanning);
            let handle = tokio::spawn(async move { worker.run().await });
            self.workers.push(WorkerHandle {
                _id: self.workers_count,
                handle,
                is_scanning,
                scan_task_sender,
            });
            self.workers_count += 1;
        }
    }

    pub(crate) fn is_worker_idle(&self) -> bool {
        self.workers.iter().any(|worker| !worker.is_scanning())
    }

    pub(crate) fn shutdown_idle_workers(&self) {
        // TODO: use take() with options on senders and handles to shutdown workers gracefully
        self.workers
            .iter()
            .filter(|worker| !worker.is_scanning())
            .for_each(|worker| {
                worker.handle.abort();
            });
    }

    pub(crate) fn add_scan_task(&self, scan_task: ScanTask) -> Result<(), ()> {
        if let Some(worker) = self.workers.iter().find(|worker| !worker.is_scanning()) {
            worker.add_scan_task(scan_task).unwrap();
        } else {
            panic!("no idle workers!")
        }

        Ok(())
    }
}

struct WorkerHandle {
    _id: usize,
    handle: JoinHandle<Result<(), ()>>,
    is_scanning: Arc<AtomicBool>,
    scan_task_sender: mpsc::UnboundedSender<ScanTask>,
}

impl WorkerHandle {
    fn is_scanning(&self) -> bool {
        self.is_scanning.load(atomic::Ordering::Acquire)
    }

    fn add_scan_task(&self, scan_task: ScanTask) -> Result<(), ()> {
        self.scan_task_sender.send(scan_task).unwrap();

        Ok(())
    }
}

struct ScanWorker<P> {
    is_scanning: Arc<AtomicBool>,
    scan_task_receiver: mpsc::UnboundedReceiver<ScanTask>,
    scan_results_sender: mpsc::UnboundedSender<(ScanRange, Result<ScanResults, ScanError>)>,
    fetch_request_sender: mpsc::UnboundedSender<FetchRequest>,
    parameters: P,
    ufvks: HashMap<AccountId, UnifiedFullViewingKey>,
}

impl<P> ScanWorker<P>
where
    P: Parameters + Sync + Send + 'static,
{
    fn new(
        scan_task_receiver: mpsc::UnboundedReceiver<ScanTask>,
        scan_results_sender: mpsc::UnboundedSender<(ScanRange, Result<ScanResults, ScanError>)>,
        fetch_request_sender: mpsc::UnboundedSender<FetchRequest>,
        parameters: P,
        ufvks: HashMap<AccountId, UnifiedFullViewingKey>,
    ) -> Self {
        Self {
            is_scanning: Arc::new(AtomicBool::new(false)),
            scan_task_receiver,
            scan_results_sender,
            fetch_request_sender,
            parameters,
            ufvks,
        }
    }

    async fn run(mut self) -> Result<(), ()> {
        while let Some(scan_task) = self.scan_task_receiver.recv().await {
            self.is_scanning.store(true, atomic::Ordering::Release);

            let scan_results = scan(
                self.fetch_request_sender.clone(),
                &self.parameters.clone(),
                &self.ufvks,
                scan_task.scan_range.clone(),
                scan_task.previous_wallet_block,
            )
            .await;

            self.scan_results_sender
                .send((scan_task.scan_range, scan_results))
                .unwrap();

            self.is_scanning.store(false, atomic::Ordering::Release);
        }

        Ok(())
    }
}

pub(crate) struct ScanTask {
    scan_range: ScanRange,
    previous_wallet_block: Option<WalletBlock>,
}

impl ScanTask {
    pub(crate) fn from_parts(
        scan_range: ScanRange,
        previous_wallet_block: Option<WalletBlock>,
    ) -> Self {
        Self {
            scan_range,
            previous_wallet_block,
        }
    }
}
