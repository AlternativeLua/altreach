use tokio::net::TcpStream;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use anyhow::Result;
use altreach_proto::{ClientMessage, ServerMessage, encode, decode, PROTOCOL_VERSION};

pub struct Connection {
    stream: TcpStream,
    buf: Vec<u8>,
}

impl Connection {
    pub async fn connect(addr: &str) -> Result<Self> {
        let stream = TcpStream::connect(addr).await?;
        Ok(Self { stream, buf: vec![] })
    }

    pub async fn send(&mut self, msg: &ClientMessage) -> Result<()> {
        let bytes = encode(msg)?;
        self.stream.write_all(&bytes).await?;
        Ok(())
    }

    pub async fn recv(&mut self) -> Result<ServerMessage> {
        loop {
            if let Some((msg, consumed)) = decode::<ServerMessage>(&self.buf)? {
                self.buf.drain(..consumed);
                return Ok(msg);
            }

            let mut tmp = [0u8; 4096];
            let n = self.stream.read(&mut tmp).await?;

            if n == 0 {
                anyhow::bail!("Connection closed with empty buffer");
            }

            self.buf.extend_from_slice(&tmp[..n]);
        }
    }
}