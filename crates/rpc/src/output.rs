//! Byte admission survives dequeue and is released only when transport drops it.
use std::sync::Arc;
use tokio::sync::{OwnedSemaphorePermit, Semaphore, mpsc};

pub struct OutputFrame {
    pub text: String,
    _bytes: OwnedSemaphorePermit,
}
impl std::ops::Deref for OutputFrame {
    type Target = str;
    fn deref(&self) -> &str {
        &self.text
    }
}
#[derive(Clone)]
pub(crate) enum Output {
    Plain(mpsc::Sender<String>),
    Bounded {
        sender: mpsc::Sender<OutputFrame>,
        budget: Arc<Semaphore>,
        limit: usize,
    },
}
pub(crate) enum Permit {
    Plain(mpsc::OwnedPermit<String>),
    Bounded {
        permit: mpsc::OwnedPermit<OutputFrame>,
        budget: Arc<Semaphore>,
        limit: usize,
    },
}
impl Output {
    pub(crate) fn bounded(sender: mpsc::Sender<OutputFrame>, limit: usize) -> Self {
        Self::Bounded {
            sender,
            budget: Arc::new(Semaphore::new(limit.max(1))),
            limit: limit.max(1),
        }
    }
    pub(crate) async fn reserve(&self) -> Result<Permit, ()> {
        Ok(match self {
            Self::Plain(sender) => {
                Permit::Plain(sender.clone().reserve_owned().await.map_err(|_| ())?)
            }
            Self::Bounded {
                sender,
                budget,
                limit,
            } => Permit::Bounded {
                permit: sender.clone().reserve_owned().await.map_err(|_| ())?,
                budget: budget.clone(),
                limit: *limit,
            },
        })
    }
}
impl Permit {
    pub(crate) async fn send(self, text: String) -> Result<(), ()> {
        match self {
            Self::Plain(permit) => {
                permit.send(text);
            }
            Self::Bounded {
                permit,
                budget,
                limit,
            } => {
                // An intact oversized response is admitted exclusively, never
                // split or silently lost. Ordinary queued + in-flight frames
                // share this budget, even after the receiver dequeues them.
                let size = text.len().max(1).min(limit).min(u32::MAX as usize) as u32;
                let bytes = budget.acquire_many_owned(size).await.map_err(|_| ())?;
                permit.send(OutputFrame {
                    text,
                    _bytes: bytes,
                });
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn dequeue_retains_byte_budget_until_write_finishes_and_cancel_releases_capacity() {
        let (sender, mut receiver) = mpsc::channel(8);
        let output = Output::bounded(sender, 10);
        output
            .reserve()
            .await
            .unwrap()
            .send("1234567890".into())
            .await
            .unwrap();
        let writing = receiver.recv().await.unwrap();
        let Output::Bounded { budget, .. } = &output else {
            unreachable!()
        };
        assert_eq!(budget.available_permits(), 0);
        let blocked = tokio::spawn({
            let output = output.clone();
            async move { output.reserve().await.unwrap().send("next".into()).await }
        });
        tokio::task::yield_now().await;
        assert!(!blocked.is_finished());
        drop(writing);
        blocked.await.unwrap().unwrap();
        let next = receiver.recv().await.unwrap();
        assert_eq!(&*next, "next");
        drop(next);
        output
            .reserve()
            .await
            .unwrap()
            .send("oversized intact response".into())
            .await
            .unwrap();
        assert_eq!(budget.available_permits(), 0);
        assert_eq!(
            &*receiver.recv().await.unwrap(),
            "oversized intact response"
        );
        assert_eq!(budget.available_permits(), 10);
    }
}
