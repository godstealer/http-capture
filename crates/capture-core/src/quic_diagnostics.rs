//! Opt-in socket metadata diagnostics. Never records datagram contents.
use std::{sync::Arc,io,net::SocketAddr,pin::Pin,task::{Context,Poll}};
use quinn::{AsyncUdpSocket,Runtime};
#[derive(Debug)]
struct Socket(Arc<dyn AsyncUdpSocket>,std::net::UdpSocket,quinn::udp::UdpSocketState);
impl AsyncUdpSocket for Socket {
    fn create_io_poller(self:Arc<Self>)->Pin<Box<dyn quinn::UdpPoller>> {self.0.clone().create_io_poller()}
    fn try_send(&self,packet:&quinn::udp::Transmit<'_>)->io::Result<()> {
        // Quinn's standard adapter converts most OS send failures into Ok(()).
        // Use the raw variant here so diagnostics expose the actual failure.
        let result=self.2.try_send((&self.1).into(),packet);
        eprintln!("quic-udp send peer={} bytes={} result={:?}",packet.destination,packet.contents.len(),result);
        result
    }
    fn poll_recv(&self,cx:&mut Context<'_>,buffers:&mut [io::IoSliceMut<'_>],meta:&mut [quinn::udp::RecvMeta])->Poll<io::Result<usize>> {
        let result=self.0.poll_recv(cx,buffers,meta);
        if let Poll::Ready(ref value)=result {eprintln!("quic-udp receive={value:?}");}
        result
    }
    fn local_addr(&self)->io::Result<SocketAddr>{self.0.local_addr()}
    fn max_transmit_segments(&self)->usize{self.0.max_transmit_segments()}
    fn max_receive_segments(&self)->usize{self.0.max_receive_segments()}
    fn may_fragment(&self)->bool{self.0.may_fragment()}
}
pub(crate) fn client(address:SocketAddr)->io::Result<quinn::Endpoint> {
    if std::env::var_os("HTTP_CAPTURE_QUIC_SOCKET_DEBUG").is_none() {return quinn::Endpoint::client(address);}
    let runtime=Arc::new(quinn::TokioRuntime);
    let raw=std::net::UdpSocket::bind(address)?;
    let state=quinn::udp::UdpSocketState::new((&raw).into())?;
    let socket=runtime.wrap_udp_socket(raw.try_clone()?)?;
    quinn::Endpoint::new_with_abstract_socket(quinn::EndpointConfig::default(),None,Arc::new(Socket(socket,raw,state)),runtime)
}
