use std::{
    future::{ready, Future, Ready},
    marker::PhantomData,
    pin::Pin,
    task::{Context, Poll},
};

use futures::{
    stream::{once, Once},
    Stream,
};
use pin_project::pin_project;
use serde::Serialize;
use serde_json::Value;
use specta::Type;

use crate::{Error, ExecError};

use super::{PinnedOption, PinnedOptionProj};

#[doc(hidden)]
pub trait RequestLayer<TMarker>: private::SealedRequestLayer<TMarker> {}

mod private {
    use super::*;

    // Markers
    #[doc(hidden)]
    pub enum StreamMarkerType {}
    #[doc(hidden)]
    pub enum FutureMarkerType {}

    pub trait SealedRequestLayer<TMarker> {
        type Result: Type;
        type Stream: Stream<Item = Result<Value, ExecError>> + Send + 'static;
        type Type;

        fn exec(self) -> Self::Stream;
    }

    impl<TMarker, T: SealedRequestLayer<TMarker>> RequestLayer<TMarker> for T {}

    // For queries and mutations

    #[doc(hidden)]
    pub enum SerializeMarker {}
    impl<T> SealedRequestLayer<SerializeMarker> for T
    where
        T: Serialize + Type,
    {
        type Result = T;
        type Stream = Once<Ready<Result<Value, ExecError>>>;
        type Type = FutureMarkerType;

        fn exec(self) -> Self::Stream {
            once(ready(
                serde_json::to_value(self).map_err(ExecError::SerializingResultErr),
            ))
        }
    }

    #[doc(hidden)]
    pub enum ResultMarker {}
    impl<T> SealedRequestLayer<ResultMarker> for Result<T, Error>
    where
        T: Serialize + Type,
    {
        type Result = T;
        type Stream = Once<Ready<Result<Value, ExecError>>>;
        type Type = FutureMarkerType;

        fn exec(self) -> Self::Stream {
            once(ready(self.map_err(ExecError::ErrResolverError).and_then(
                |v| serde_json::to_value(v).map_err(ExecError::SerializingResultErr),
            )))
        }
    }

    #[doc(hidden)]
    pub enum FutureSerializeMarker {}
    impl<TFut, T> SealedRequestLayer<FutureSerializeMarker> for TFut
    where
        TFut: Future<Output = T> + Send + 'static,
        T: Serialize + Type + Send + 'static,
    {
        type Result = T;
        type Stream = Once<FutureSerializeFuture<TFut, T>>;
        type Type = FutureMarkerType;

        fn exec(self) -> Self::Stream {
            once(FutureSerializeFuture(self, PhantomData))
        }
    }

    #[pin_project(project = FutureSerializeFutureProj)]
    pub struct FutureSerializeFuture<TFut, T>(#[pin] TFut, PhantomData<T>);

    impl<TFut, T> Future for FutureSerializeFuture<TFut, T>
    where
        TFut: Future<Output = T> + Send + 'static,
        T: Serialize + Type + Send + 'static,
    {
        type Output = Result<Value, ExecError>;

        fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
            let this = self.project();
            match this.0.poll(cx) {
                Poll::Ready(v) => {
                    Poll::Ready(serde_json::to_value(v).map_err(ExecError::SerializingResultErr))
                }
                Poll::Pending => Poll::Pending,
            }
        }
    }

    #[doc(hidden)]
    pub enum FutureResultMarker {}
    impl<TFut, T> SealedRequestLayer<FutureResultMarker> for TFut
    where
        TFut: Future<Output = Result<T, Error>> + Send + 'static,
        T: Serialize + Type + Send + 'static,
    {
        type Result = T;
        type Stream = Once<FutureSerializeResultFuture<TFut, T>>;
        type Type = FutureMarkerType;

        fn exec(self) -> Self::Stream {
            once(FutureSerializeResultFuture(self, PhantomData))
        }
    }

    #[pin_project(project = FutureSerializeResultFutureProj)]
    pub struct FutureSerializeResultFuture<TFut, T>(#[pin] TFut, PhantomData<T>);

    impl<TFut, T> Future for FutureSerializeResultFuture<TFut, T>
    where
        TFut: Future<Output = Result<T, Error>> + Send + 'static,
        T: Serialize + Type + Send + 'static,
    {
        type Output = Result<Value, ExecError>;

        fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
            let this = self.project();
            match this.0.poll(cx) {
                Poll::Ready(v) => {
                    Poll::Ready(v.map_err(ExecError::ErrResolverError).and_then(|v| {
                        serde_json::to_value(v).map_err(ExecError::SerializingResultErr)
                    }))
                }
                Poll::Pending => Poll::Pending,
            }
        }
    }

    // For subscriptions

    #[doc(hidden)]
    pub enum StreamMarker {}
    impl<TStream, T> SealedRequestLayer<StreamMarker> for TStream
    where
        TStream: Stream<Item = T> + Send + Sync + 'static,
        T: Serialize + Type,
    {
        type Result = T;
        type Stream = MapStream<TStream>;
        type Type = StreamMarkerType;

        fn exec(self) -> Self::Stream {
            MapStream(None, PinnedOption::Some(self), |v| {
                serde_json::to_value(v).map_err(ExecError::SerializingResultErr)
            })
        }
    }

    #[doc(hidden)]
    pub enum ResultStreamMarker {}
    impl<TStream, T> SealedRequestLayer<ResultStreamMarker> for Result<TStream, Error>
    where
        TStream: Stream<Item = T> + Send + Sync + 'static,
        T: Serialize + Type,
    {
        type Result = T;
        type Stream = MapStream<TStream>;
        type Type = StreamMarker;

        fn exec(self) -> Self::Stream {
            let (err, stream) = match self {
                Ok(v) => (None, PinnedOption::Some(v)),
                Err(err) => (Some(ExecError::ErrResolverError(err)), PinnedOption::None),
            };

            MapStream(err, stream, |v| {
                serde_json::to_value(v).map_err(ExecError::SerializingResultErr)
            })
        }
    }

    #[doc(hidden)]
    pub enum FutureStreamMarker {}
    impl<TFut, TStream, T> SealedRequestLayer<FutureStreamMarker> for TFut
    where
        TFut: Future<Output = TStream> + Send + 'static,
        TStream: Stream<Item = T> + Send + Sync + 'static,
        T: Serialize + Type,
    {
        type Result = T;
        type Stream = FutureMapStream<TFut, TStream>;
        type Type = StreamMarker;

        fn exec(self) -> Self::Stream {
            FutureMapStream(
                None,
                PinnedOption::Some(self),
                PinnedOption::None,
                |s| Ok(s),
                |v| serde_json::to_value(v).map_err(ExecError::SerializingResultErr),
            )
        }
    }

    #[doc(hidden)]
    pub enum FutureResultStreamMarker {}
    impl<TFut, TStream, T> SealedRequestLayer<FutureResultStreamMarker> for TFut
    where
        TFut: Future<Output = Result<TStream, Error>> + Send + 'static,
        TStream: Stream<Item = T> + Send + Sync + 'static,
        T: Serialize + Type,
    {
        type Result = T;
        type Stream = FutureMapStream<TFut, TStream>;
        type Type = StreamMarker;

        fn exec(self) -> Self::Stream {
            FutureMapStream(
                None,
                PinnedOption::Some(self),
                PinnedOption::None,
                |s| s.map_err(ExecError::ErrResolverError),
                |v| serde_json::to_value(v).map_err(ExecError::SerializingResultErr),
            )
        }
    }

    #[pin_project(project = MapStreamProj)]
    pub struct MapStream<S: Stream>(
        Option<ExecError>,
        #[pin] PinnedOption<S>,
        fn(S::Item) -> Result<Value, ExecError>,
    );

    impl<S: Stream> Stream for MapStream<S> {
        type Item = Result<Value, ExecError>;

        fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
            let this = self.project();

            if let Some(err) = this.0.take() {
                return Poll::Ready(Some(Err(err)));
            }

            match this.1.project() {
                PinnedOptionProj::Some(s) => match s.poll_next(cx) {
                    Poll::Ready(result) => Poll::Ready(result.map(this.2)),
                    Poll::Pending => Poll::Pending,
                },
                PinnedOptionProj::None => Poll::Ready(None),
            }
        }

        fn size_hint(&self) -> (usize, Option<usize>) {
            match &self.1 {
                PinnedOption::Some(s) => s.size_hint(),
                PinnedOption::None => (0, Some(0)),
            }
        }
    }

    #[pin_project(project = FutureMapStreamProj)]
    pub struct FutureMapStream<F: Future, S: Stream>(
        Option<ExecError>,
        #[pin] PinnedOption<F>,
        #[pin] PinnedOption<S>,
        fn(F::Output) -> Result<S, ExecError>,
        fn(S::Item) -> Result<Value, ExecError>,
    );

    impl<F: Future, S: Stream> Stream for FutureMapStream<F, S> {
        type Item = Result<Value, ExecError>;

        fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
            let mut this = self.as_mut().project();
            if let Some(err) = this.0.take() {
                return Poll::Ready(Some(Err(err)));
            }

            match this.1.as_mut().project() {
                PinnedOptionProj::Some(s) => match s.poll(cx) {
                    Poll::Ready(result) => {
                        this.1.set(PinnedOption::None);
                        match (this.3)(result) {
                            Ok(v) => this.2.set(PinnedOption::Some(v)),
                            Err(err) => return Poll::Ready(Some(Err(err))),
                        }
                    }
                    Poll::Pending => return Poll::Pending,
                },
                PinnedOptionProj::None => {}
            }

            match this.2.project() {
                PinnedOptionProj::Some(s) => match s.poll_next(cx) {
                    Poll::Ready(result) => Poll::Ready(result.map(this.4)),
                    Poll::Pending => Poll::Pending,
                },
                PinnedOptionProj::None => Poll::Ready(None),
            }
        }

        fn size_hint(&self) -> (usize, Option<usize>) {
            if let PinnedOption::Some(_) = self.1 {
                return (0, None);
            }

            match &self.2 {
                PinnedOption::Some(s) => s.size_hint(),
                PinnedOption::None => (0, Some(0)),
            }
        }
    }
}

pub(crate) use private::{FutureMarkerType, SealedRequestLayer, StreamMarkerType};
