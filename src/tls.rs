use crate::error::Error;

use hyper::{
    client::connect::{
        Connected,
        Connection,
        HttpConnector
    },
    service::Service,
};
use std::{
    fmt,
    future::Future,
    io::IoSlice,
    marker::Unpin,
    pin::Pin,
    task::{
        Context,
        Poll,
    },
};
use tokio::io::{
    AsyncRead,
    AsyncWrite,
    ReadBuf,
};
use tokio_native_tls::{
    self,
    TlsConnector,
};


// This shouldn't be necessary because hyper-tls is already a thing, but
// hyper-tls does not have a way to enforce a stream to be interpreted as
// https traffic. It has a "force_https" flag, but all that will do is cause
// an error if the scheme is not "https". For this, we're going to be using
// a scheme of "wss" which is still https, so using the https flag on hyper-tls
// will mean that we'll just get an error. If we just don't use the flag, we'll
// just be given a regular Http stream, but our traffic is https, so had to
// create my own TlsStream and HttpsConnector.
#[derive(Debug)]
pub struct TlsStream<T>(tokio_native_tls::TlsStream<T>);
impl<T: AsyncRead + AsyncWrite + Connection + Unpin> Connection for TlsStream<T> {
    fn connected(&self) -> Connected {
        self.0.get_ref().get_ref().get_ref().connected()
    }
}
impl<T: AsyncRead + AsyncWrite + Unpin> AsyncRead for TlsStream<T> {
    #[inline]
    fn poll_read(self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &mut ReadBuf<'_>) -> Poll<Result<(), std::io::Error>> {
        Pin::new(&mut self.get_mut().0).poll_read(cx, buf)
    }
}

impl<T: AsyncWrite + AsyncRead + Unpin> AsyncWrite for TlsStream<T> {
    #[inline]
    fn poll_write(self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &[u8]) -> Poll<Result<usize, std::io::Error>> {
        Pin::new(&mut self.get_mut().0).poll_write(cx, buf)
    }

    #[inline]
    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), std::io::Error>> {
        Pin::new(&mut self.get_mut().0).poll_flush(cx)
    }

    #[inline]
    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), std::io::Error>> {
        Pin::new(&mut self.get_mut().0).poll_shutdown(cx)
    }

    #[inline]
    fn poll_write_vectored(self: Pin<&mut Self>, cx: &mut Context<'_>, bufs: &[IoSlice<'_>]) -> Poll<Result<usize, std::io::Error>> {
        Pin::new(&mut self.get_mut().0).poll_write_vectored(cx, bufs)
    }
}

#[derive(Clone)]
pub struct HttpsConnector<T> {
    http: T,
    tls: TlsConnector,
}

impl HttpsConnector<HttpConnector> {
    pub fn new() -> Result<Self, native_tls::Error> {
        native_tls::TlsConnector::new().map(|tls| HttpsConnector::new_(TlsConnector::from(tls)))
    }
    fn new_(tls: TlsConnector) -> Self {
        let mut http = HttpConnector::new();
        http.enforce_http(false);
        HttpsConnector {
            http,
            tls
        }
    }
}

impl<T> Service<hyper::Uri> for HttpsConnector<T>
    where T: Service<hyper::Uri>,
          T::Response: AsyncRead + AsyncWrite + Send + Unpin,
          T::Future: Send + 'static,
          T::Error: Into<Box<dyn std::error::Error + Send + Sync>> + Send + Sync
{
    type Response = TlsStream<T::Response>;
    type Future = HttpsConnecting<T::Response>;
    type Error = Error;

    fn poll_ready(&mut self, cx: &mut Context) -> Poll<Result<(), Self::Error>> {
        match self.http.poll_ready(cx) {
            Poll::Ready(Ok(())) => Poll::Ready(Ok(())),
            Poll::Ready(Err(e)) => Poll::Ready(Err(Error::from(e.into()))),
            Poll::Pending => Poll::Pending
        }
    }
    fn call(&mut self, dst: hyper::Uri) -> Self::Future {
        // This is a little annoying, there doesn't appear to be a way to easily
        // just change the port of a Uri. This is an issue because, the
        // underlying HttpConnector will just look at the scheme to determine
        // the port unless it's been specifically set, and it will *only* use
        // port 443 if the scheme is Https, ignoring the wss that we're using
        // here.
        //
        // Instead we just try to build the same Uri, overwriting the port
        // unless the port has already specifically been set.
        let values = if let (None, Some(host)) = (dst.port(), dst.host()) {
            let mut dst_builder = hyper::Uri::builder();
            if let Some(s) = dst.scheme() {
                dst_builder = dst_builder.scheme(s.clone());
            }
            dst_builder = dst_builder.authority(&*format!("{}:{}", host, 443));
            if let Some(p) = dst.path_and_query() {
                dst_builder = dst_builder.path_and_query(p.clone());
            }
            dst_builder.build()
                .map(|dst| (host.to_owned(), self.http.call(dst), self.tls.clone()))
        } else {
            Ok((dst.host().unwrap_or("").to_owned(), self.http.call(dst), self.tls.clone()))
        };
        let fut = async move {
            match values {
                Ok((host, connecting, tls)) => {
                    match connecting.await {
                        Ok(tcp) => tls.connect(&host, tcp).await.map(TlsStream).map_err(Into::into),
                        Err(e) => Err(<Error as From<_>>::from(e.into())),
                    }
                },
                Err(e) => Err(<Error as From<http::Error>>::from(e)),
            }
        };
        HttpsConnecting(Box::pin(fut))
    }
}

type BoxedFut<T> =
    Pin<Box<dyn Future<Output = Result<TlsStream<T>, Error>> + Send>>;

/// A Future representing work to connect to a URL, and a TLS handshake.
pub struct HttpsConnecting<T>(BoxedFut<T>);

impl<T: AsyncRead + AsyncWrite + Unpin> Future for HttpsConnecting<T> {
    type Output = Result<TlsStream<T>, Error>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        Pin::new(&mut self.0).poll(cx)
    }
}

impl<T> fmt::Debug for HttpsConnecting<T> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.pad("HttpsConnecting")
    }
}
