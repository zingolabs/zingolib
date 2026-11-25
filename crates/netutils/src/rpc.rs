#![forbid(unsafe_code)]

use std::future::Future;
use std::time::Duration;

pub(crate) fn timed<Message>(message: Message, timeout: Duration) -> tonic::Request<Message> {
    let mut request = tonic::Request::new(message);
    request.set_timeout(timeout);
    request
}

pub(crate) async fn reply<Reply>(
    response: impl Future<Output = Result<tonic::Response<Reply>, tonic::Status>>,
) -> Result<Reply, tonic::Status> {
    Ok(response.await?.into_inner())
}

pub(crate) async fn call<Message, Reply, Response>(
    message: Message,
    timeout: Duration,
    method: impl FnOnce(tonic::Request<Message>) -> Response,
) -> Result<Reply, tonic::Status>
where
    Response: Future<Output = Result<tonic::Response<Reply>, tonic::Status>>,
{
    reply(method(timed(message, timeout))).await
}
