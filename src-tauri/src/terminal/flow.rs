use std::{
    collections::BTreeMap,
    sync::{Condvar, Mutex},
};

use crate::filesystem::ExplorerError;

#[derive(Debug)]
struct OutputWindowState {
    next_sequence: u64,
    last_emitted_sequence: Option<u64>,
    in_flight: BTreeMap<u64, usize>,
    in_flight_bytes: usize,
    closed: bool,
}

#[derive(Debug)]
pub struct OutputWindow {
    max_in_flight_bytes: usize,
    state: Mutex<OutputWindowState>,
    capacity_available: Condvar,
}

impl OutputWindow {
    pub fn new(max_in_flight_bytes: usize) -> Self {
        Self {
            max_in_flight_bytes,
            state: Mutex::new(OutputWindowState {
                next_sequence: 0,
                last_emitted_sequence: None,
                in_flight: BTreeMap::new(),
                in_flight_bytes: 0,
                closed: false,
            }),
            capacity_available: Condvar::new(),
        }
    }

    pub fn reserve(&self, byte_count: usize) -> Result<u64, ExplorerError> {
        if byte_count == 0 || byte_count > self.max_in_flight_bytes {
            return Err(ExplorerError::InvalidReference);
        }

        let mut state = self
            .state
            .lock()
            .map_err(|_| ExplorerError::StateUnavailable)?;
        while !state.closed
            && state
                .in_flight_bytes
                .checked_add(byte_count)
                .is_none_or(|total| total > self.max_in_flight_bytes)
        {
            state = self
                .capacity_available
                .wait(state)
                .map_err(|_| ExplorerError::StateUnavailable)?;
        }
        if state.closed {
            return Err(ExplorerError::Cancelled);
        }

        let sequence = state.next_sequence;
        state.next_sequence = state.next_sequence.checked_add(1).ok_or_else(|| {
            ExplorerError::Unexpected("Terminal output sequence exhausted.".into())
        })?;
        state.last_emitted_sequence = Some(sequence);
        state.in_flight.insert(sequence, byte_count);
        state.in_flight_bytes += byte_count;
        Ok(sequence)
    }

    pub fn acknowledge(&self, sequence: u64) -> Result<(), ExplorerError> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| ExplorerError::StateUnavailable)?;
        if state
            .last_emitted_sequence
            .is_none_or(|last_emitted| sequence > last_emitted)
        {
            return Err(ExplorerError::InvalidReference);
        }

        let acknowledged = state
            .in_flight
            .range(..=sequence)
            .map(|(sequence, byte_count)| (*sequence, *byte_count))
            .collect::<Vec<_>>();
        if acknowledged.is_empty() {
            return Ok(());
        }
        for (acknowledged_sequence, byte_count) in acknowledged {
            state.in_flight.remove(&acknowledged_sequence);
            state.in_flight_bytes -= byte_count;
        }
        self.capacity_available.notify_all();
        Ok(())
    }

    pub fn close(&self) {
        if let Ok(mut state) = self.state.lock() {
            state.closed = true;
            self.capacity_available.notify_all();
        }
    }

    #[cfg(test)]
    fn in_flight_bytes(&self) -> usize {
        self.state
            .lock()
            .map(|state| state.in_flight_bytes)
            .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use std::{
        sync::{mpsc, Arc},
        thread,
        time::Duration,
    };

    use super::OutputWindow;

    #[test]
    fn acknowledgements_release_only_contiguous_emitted_ranges() {
        let window = OutputWindow::new(12);
        assert_eq!(window.reserve(4).unwrap(), 0);
        assert_eq!(window.reserve(5).unwrap(), 1);
        assert_eq!(window.in_flight_bytes(), 9);

        window.acknowledge(0).unwrap();
        assert_eq!(window.in_flight_bytes(), 5);
        window.acknowledge(0).unwrap();
        assert_eq!(window.in_flight_bytes(), 5);
        assert!(window.acknowledge(2).is_err());
    }

    #[test]
    fn producer_waits_for_bounded_capacity() {
        let window = Arc::new(OutputWindow::new(4));
        assert_eq!(window.reserve(4).unwrap(), 0);
        let (sender, receiver) = mpsc::channel();
        let worker_window = window.clone();
        let worker = thread::spawn(move || {
            sender.send(worker_window.reserve(1)).unwrap();
        });

        assert!(receiver.recv_timeout(Duration::from_millis(50)).is_err());
        window.acknowledge(0).unwrap();
        assert_eq!(
            receiver
                .recv_timeout(Duration::from_secs(1))
                .unwrap()
                .unwrap(),
            1
        );
        worker.join().unwrap();
    }

    #[test]
    fn close_unblocks_a_waiting_producer() {
        let window = Arc::new(OutputWindow::new(1));
        window.reserve(1).unwrap();
        let (sender, receiver) = mpsc::channel();
        let worker_window = window.clone();
        let worker = thread::spawn(move || {
            sender.send(worker_window.reserve(1)).unwrap();
        });

        window.close();
        assert!(receiver
            .recv_timeout(Duration::from_secs(1))
            .unwrap()
            .is_err());
        worker.join().unwrap();
    }
}
