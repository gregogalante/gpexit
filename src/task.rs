use anyhow::{bail, Result};
use serde::Serialize;
use serde_json::Value;
use std::{
    collections::VecDeque,
    panic::{catch_unwind, AssertUnwindSafe},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Condvar, Mutex,
    },
};

#[derive(Clone, Debug, Serialize)]
pub struct TaskStatus {
    pub running: bool,
    pub status: String,
    pub log: VecDeque<String>,
    pub done: u64,
    pub total: u64,
    pub cancel_requested: bool,
    pub result: Option<Value>,
}

impl Default for TaskStatus {
    fn default() -> Self {
        Self {
            running: false,
            status: "Pronto".into(),
            log: VecDeque::new(),
            done: 0,
            total: 0,
            cancel_requested: false,
            result: None,
        }
    }
}

#[derive(Clone, Default)]
pub struct TaskManager {
    state: Arc<(Mutex<TaskStatus>, Condvar)>,
    cancelled: Arc<AtomicBool>,
}

#[derive(Clone, Default)]
pub struct TaskContext {
    manager: TaskManager,
}

impl TaskContext {
    pub fn check_cancelled(&self) -> Result<()> {
        if self.is_cancelled() {
            bail!("Operation cancelled");
        }
        Ok(())
    }
    pub fn is_cancelled(&self) -> bool {
        self.manager.cancelled.load(Ordering::Relaxed)
    }
    pub fn log(&self, message: impl Into<String>) {
        let mut state = self
            .manager
            .state
            .0
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if state.log.len() >= 500 {
            state.log.pop_front();
        }
        state.log.push_back(message.into());
    }
    pub fn progress(&self, done: u64, total: u64) {
        let mut state = self
            .manager
            .state
            .0
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        state.done = done;
        state.total = total;
    }
}

impl TaskManager {
    pub fn status(&self) -> TaskStatus {
        self.state
            .0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }
    pub fn running(&self) -> bool {
        self.status().running
    }
    pub fn cancel(&self) {
        let mut state = self.state.0.lock().unwrap_or_else(|e| e.into_inner());
        if state.running {
            self.cancelled.store(true, Ordering::Relaxed);
            state.cancel_requested = true;
        }
    }
    pub fn wait(&self) {
        let (lock, finished) = &*self.state;
        let mut state = lock.lock().unwrap_or_else(|e| e.into_inner());
        while state.running {
            state = finished.wait(state).unwrap_or_else(|e| e.into_inner());
        }
    }
    pub fn start<F>(&self, operation: F) -> Result<()>
    where
        F: FnOnce(TaskContext) -> Result<Value> + Send + 'static,
    {
        {
            let mut state = self.state.0.lock().unwrap_or_else(|e| e.into_inner());
            if state.running {
                bail!("Un task è già in esecuzione.");
            }
            self.cancelled.store(false, Ordering::Relaxed);
            *state = TaskStatus {
                running: true,
                status: "In esecuzione".into(),
                ..TaskStatus::default()
            };
        }
        let context = TaskContext {
            manager: self.clone(),
        };
        let manager = self.clone();
        let spawned = std::thread::Builder::new()
            .name("gpexit-task".into())
            .spawn(move || {
                let outcome = catch_unwind(AssertUnwindSafe(|| operation(context.clone())));
                let (status, result) = match outcome {
                    Ok(Ok(result)) => {
                        context.log(result.to_string());
                        let status = if context.is_cancelled() {
                            "Interrotto: i file completati sono conservati"
                        } else {
                            "Completato"
                        };
                        (status.to_string(), Some(result))
                    }
                    Ok(Err(error)) => {
                        context.log(format!("{error:#}"));
                        let status = if context.is_cancelled() {
                            "Interrotto: i file completati sono conservati".into()
                        } else {
                            "Fallito".into()
                        };
                        (status, None)
                    }
                    Err(_) => {
                        context.log("Unexpected task failure; completed files remain available.");
                        ("Fallito".into(), None)
                    }
                };
                let mut state = manager.state.0.lock().unwrap_or_else(|e| e.into_inner());
                state.status = status;
                state.result = result;
                state.running = false;
                manager.state.1.notify_all();
            });
        if let Err(error) = spawned {
            let mut state = self.state.0.lock().unwrap_or_else(|e| e.into_inner());
            state.running = false;
            state.status = "Fallito".into();
            self.state.1.notify_all();
            return Err(error.into());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cancellation_and_serialization() -> Result<()> {
        let tasks = TaskManager::default();
        tasks.start(|ctx| {
            while !ctx.is_cancelled() {
                std::thread::yield_now();
            }
            ctx.check_cancelled()?;
            Ok(Value::Null)
        })?;
        assert!(tasks.start(|_| Ok(Value::Null)).is_err());
        tasks.cancel();
        tasks.wait();
        assert!(!tasks.running());
        assert!(tasks.status().status.starts_with("Interrotto"));
        Ok(())
    }
}
