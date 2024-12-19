//! Wait for the first of several futures to complete.

use core::future::Future;
use core::pin::Pin;
use core::task::{Context, Poll};

/// Result for [`select`].
#[derive(Debug, Clone)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub enum Either<A, B> {
    /// First future finished first.
    First(A),
    /// Second future finished first.
    Second(B),
}

/// Wait for one of two futures to complete.
///
/// This function returns a new future which polls all the futures.
/// When one of them completes, it will complete with its result value.
///
/// The other future is dropped.
pub fn poll_select<A, B, Output, F: FnMut(Either<A::Output, B::Output>) -> Poll<Output>>(
    a: A,
    b: B,
    f: F,
) -> Select<A, B, F>
where
    A: Future,
    B: Future,
{
    Select { a, b, f }
}

/// Future for the [`select`] function.
#[derive(Debug)]
#[must_use = "futures do nothing unless you `.await` or poll them"]
pub struct Select<A, B, F> {
    a: A,
    b: B,
    f: F,
}

impl<A: Unpin, B: Unpin, F: Unpin> Unpin for Select<A, B, F> {}

impl<A, B, Output, F> Future for Select<A, B, F>
where
    A: Future,
    B: Future,
    F: FnMut(Either<A::Output, B::Output>) -> Poll<Output>,
{
    type Output = Output;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = unsafe { self.get_unchecked_mut() };
        let a = unsafe { Pin::new_unchecked(&mut this.a) };
        let b = unsafe { Pin::new_unchecked(&mut this.b) };
        if let Poll::Ready(x) = a.poll(cx) {
            if let Poll::Ready(res) = (this.f)(Either::First(x)) {
                return Poll::Ready(res);
            }
        }
        if let Poll::Ready(x) = b.poll(cx) {
            if let Poll::Ready(res) = (this.f)(Either::Second(x)) {
                return Poll::Ready(res);
            }
        }
        Poll::Pending
    }
}
