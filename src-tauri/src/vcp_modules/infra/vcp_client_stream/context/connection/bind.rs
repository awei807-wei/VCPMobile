use std::future::Future;
use std::time::Duration;

/// 以有界超时执行持久化绑定。helper 确认 generation 后，所有绑定失败（包括
/// 超时）都必须先执行传入的精确 stop。
pub async fn bind_generation_with_timeout<Fut, Stop, StopFut>(
    timeout: Duration,
    bind: Fut,
    stop: Stop,
) -> Result<(), String>
where
    Fut: Future<Output = Result<(), String>>,
    Stop: Fn() -> StopFut,
    StopFut: Future<Output = ()>,
{
    match tokio::time::timeout(timeout, bind).await {
        Ok(Ok(())) => Ok(()),
        Ok(Err(error)) => {
            stop().await;
            Err(error)
        }
        Err(_) => {
            stop().await;
            Err("绑定 helper generation 超时".to_string())
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum BindGenerationAttempt {
    Bound,
    Cancelled,
    Failed(String),
}

/// 绑定 helper generation 的生产取消/超时编排。
pub async fn bind_generation_with_timeout_or_cancel<Fut, Stop, StopFut>(
    timeout: Duration,
    abort_rx: &mut tokio::sync::oneshot::Receiver<()>,
    bind: Fut,
    stop: Stop,
) -> BindGenerationAttempt
where
    Fut: Future<Output = Result<(), String>>,
    Stop: Fn() -> StopFut + Clone,
    StopFut: Future<Output = ()>,
{
    tokio::select! {
        _ = abort_rx => {
            stop().await;
            BindGenerationAttempt::Cancelled
        }
        result = bind_generation_with_timeout(timeout, bind, stop.clone()) => {
            match result {
                Ok(()) => BindGenerationAttempt::Bound,
                Err(error) => BindGenerationAttempt::Failed(error),
            }
        }
    }
}
