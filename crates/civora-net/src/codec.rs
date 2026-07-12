//! Length-framed request-response codecs for the two Civora request protocols:
//! `/civora/sync/1` (world + governance snapshots) and `/civora/fetch/1`
//! (content-addressed blob fetch).
//!
//! Frames are `len (u32 LE) || payload`, with the payload decoded by the
//! canonical [`wire`] codecs. Reads enforce each protocol's size caps before
//! allocating, so a malicious peer cannot ask us to buffer more than the cap
//! for that protocol. The two protocols carry independent caps: a blob fetch
//! never inherits sync's 64 MiB response ceiling and vice versa.

use std::io;

use async_trait::async_trait;
use futures::prelude::*;
use libp2p::StreamProtocol;
use libp2p::request_response;

use crate::wire::{self, FetchRequest, FetchResponse, SyncRequest, SyncResponse};

/// Protocol name, versioned independently of gossip topics.
pub const SYNC_PROTOCOL: StreamProtocol = StreamProtocol::new("/civora/sync/1");

/// Content-fetch protocol name, versioned independently of the sync protocol so
/// either can evolve alone.
pub const FETCH_PROTOCOL: StreamProtocol = StreamProtocol::new("/civora/fetch/1");

#[derive(Clone, Default)]
pub struct SyncCodec;

async fn read_frame<T>(io: &mut T, max_len: usize) -> io::Result<Vec<u8>>
where
    T: AsyncRead + Unpin + Send,
{
    let mut len = [0u8; 4];
    io.read_exact(&mut len).await?;
    let len = u32::from_le_bytes(len) as usize;
    if len > max_len {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("frame of {len} bytes exceeds cap of {max_len}"),
        ));
    }
    let mut payload = vec![0u8; len];
    io.read_exact(&mut payload).await?;
    Ok(payload)
}

async fn write_frame<T>(io: &mut T, payload: Vec<u8>) -> io::Result<()>
where
    T: AsyncWrite + Unpin + Send,
{
    io.write_all(&(payload.len() as u32).to_le_bytes()).await?;
    io.write_all(&payload).await?;
    io.close().await
}

fn invalid(what: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, format!("malformed {what}"))
}

#[async_trait]
impl request_response::Codec for SyncCodec {
    type Protocol = StreamProtocol;
    type Request = SyncRequest;
    type Response = SyncResponse;

    async fn read_request<T>(&mut self, _: &Self::Protocol, io: &mut T) -> io::Result<Self::Request>
    where
        T: AsyncRead + Unpin + Send,
    {
        let payload = read_frame(io, wire::MAX_REQUEST_BYTES).await?;
        SyncRequest::decode(&payload).ok_or_else(|| invalid("sync request"))
    }

    async fn read_response<T>(
        &mut self,
        _: &Self::Protocol,
        io: &mut T,
    ) -> io::Result<Self::Response>
    where
        T: AsyncRead + Unpin + Send,
    {
        let payload = read_frame(io, wire::MAX_RESPONSE_BYTES).await?;
        SyncResponse::decode(&payload).ok_or_else(|| invalid("sync response"))
    }

    async fn write_request<T>(
        &mut self,
        _: &Self::Protocol,
        io: &mut T,
        request: Self::Request,
    ) -> io::Result<()>
    where
        T: AsyncWrite + Unpin + Send,
    {
        let mut payload = Vec::new();
        request.encode(&mut payload);
        write_frame(io, payload).await
    }

    async fn write_response<T>(
        &mut self,
        _: &Self::Protocol,
        io: &mut T,
        response: Self::Response,
    ) -> io::Result<()>
    where
        T: AsyncWrite + Unpin + Send,
    {
        let mut payload = Vec::new();
        response.encode(&mut payload);
        write_frame(io, payload).await
    }
}

#[derive(Clone, Default)]
pub struct FetchCodec;

#[async_trait]
impl request_response::Codec for FetchCodec {
    type Protocol = StreamProtocol;
    type Request = FetchRequest;
    type Response = FetchResponse;

    async fn read_request<T>(&mut self, _: &Self::Protocol, io: &mut T) -> io::Result<Self::Request>
    where
        T: AsyncRead + Unpin + Send,
    {
        let payload = read_frame(io, wire::MAX_FETCH_REQUEST_BYTES).await?;
        FetchRequest::decode(&payload).ok_or_else(|| invalid("fetch request"))
    }

    async fn read_response<T>(
        &mut self,
        _: &Self::Protocol,
        io: &mut T,
    ) -> io::Result<Self::Response>
    where
        T: AsyncRead + Unpin + Send,
    {
        let payload = read_frame(io, wire::MAX_FETCH_RESPONSE_BYTES).await?;
        FetchResponse::decode(&payload).ok_or_else(|| invalid("fetch response"))
    }

    async fn write_request<T>(
        &mut self,
        _: &Self::Protocol,
        io: &mut T,
        request: Self::Request,
    ) -> io::Result<()>
    where
        T: AsyncWrite + Unpin + Send,
    {
        let mut payload = Vec::new();
        request.encode(&mut payload);
        write_frame(io, payload).await
    }

    async fn write_response<T>(
        &mut self,
        _: &Self::Protocol,
        io: &mut T,
        response: Self::Response,
    ) -> io::Result<()>
    where
        T: AsyncWrite + Unpin + Send,
    {
        let mut payload = Vec::new();
        response.encode(&mut payload);
        write_frame(io, payload).await
    }
}
