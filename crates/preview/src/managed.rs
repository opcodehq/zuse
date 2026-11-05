//! Managed preview transport. Authentication belongs to the host gateway; this
//! lane never shares the execution socket or accepts caller-supplied ports.
use crate::{catalog::Catalog, discovery, mux::{self, BoxIo, Connector, Mux, Transport}};
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
use std::{net::{IpAddr, Ipv4Addr, SocketAddr}, path::PathBuf, sync::Arc};
use tokio::sync::{Mutex, mpsc};
use tokio_util::sync::CancellationToken;

pub const PROTOCOL: &str = "zuse-managed-preview-v1";
pub const MAX_FRAME: usize = 8197;
pub const MAX_CATALOG: usize = 65536;
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all="camelCase", deny_unknown_fields)]
pub struct Binding {
    pub workspace_id: String,
    pub generation: i64,
    pub gateway_epoch: i64,
    pub storage_incarnation_id: String,
}
impl Binding {
    pub fn validate(&self) -> Result<()> {
        ensure!(self.generation>0 && self.gateway_epoch>0,"preview_binding_invalid");
        for value in [&self.workspace_id,&self.storage_incarnation_id] {
            ensure!(!value.is_empty() && value.len()<=128 && value.bytes().all(|b|b.is_ascii_alphanumeric() || b"_-".contains(&b)),"preview_binding_invalid");
        }
        Ok(())
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all="camelCase", deny_unknown_fields)]
pub struct Service { pub id: String, pub name: String, pub port: u16 }
impl Service {
    pub fn validate(&self)->Result<()> {
        ensure!(uuid::Uuid::parse_str(&self.id).is_ok() && self.id.len()==36,"preview_service_id_invalid");
        ensure!(!self.name.is_empty() && self.name.len()<=128 && !self.name.chars().any(char::is_control),"preview_service_name_invalid");
        ensure!(self.port>=1024 && self.port!=zuse_protocol::PREVIEW_PROXY_PORT,"preview_service_port_reserved");Ok(())
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all="camelCase", deny_unknown_fields)]
pub struct Announcement { pub version: u8, pub binding: Binding, pub services: Vec<Service> }
impl Announcement {
    pub fn validate(&self)->Result<()> {
        ensure!(self.version==1 && self.services.len()<=64,"preview_announcement_invalid");self.binding.validate()?;
        let mut ids=std::collections::HashSet::new();
        for service in &self.services {service.validate()?;ensure!(ids.insert(&service.id),"preview_service_duplicate");} Ok(())
    }
}
struct ChannelTransport { outgoing: mpsc::Sender<Vec<u8>>, incoming: Mutex<mpsc::Receiver<Vec<u8>>> }
#[async_trait::async_trait]
impl Transport for ChannelTransport {
    async fn send(&self, bytes:&[u8])->Result<()> {ensure!((5..=MAX_FRAME).contains(&bytes.len()),"preview_frame_invalid");self.outgoing.send(bytes.to_vec()).await?;Ok(())}
    async fn receive(&self)->Result<Vec<u8>> {let bytes=self.incoming.lock().await.recv().await.ok_or_else(||anyhow::anyhow!("preview_transport_closed"))?;ensure!((5..=MAX_FRAME).contains(&bytes.len()),"preview_frame_invalid");Ok(bytes)}
}
/// The caller authenticates a separate bulk socket and owns its cancellation.
/// Only 32 frames (at most 256KiB) can queue in each direction.
pub fn channel(connector:Arc<dyn Connector>,initiator:bool,stop:CancellationToken)->(Mux,mpsc::Sender<Vec<u8>>,mpsc::Receiver<Vec<u8>>) {
    let (outgoing, output)=mpsc::channel(32);let (input,incoming)=mpsc::channel(32);
    let transport=Arc::new(ChannelTransport{outgoing,incoming:Mutex::new(incoming)});
    (Mux::start(transport,connector,initiator,stop),input,output)
}
pub struct RejectConnector;
#[async_trait::async_trait]
impl Connector for RejectConnector {async fn connect(&self,_:&str)->Result<BoxIo>{anyhow::bail!("preview_reverse_open_rejected")}}

/// Excludes login callbacks, other checkouts, runtime ports and arbitrary targets.
pub struct TaskConnector { catalog:Catalog, checkout:PathBuf, blocked_ports:Vec<u16> }
impl TaskConnector {
    pub fn new(catalog:Catalog, checkout:PathBuf,blocked_ports:Vec<u16>)->Result<Self>{Ok(Self{catalog,checkout:checkout.canonicalize()?,blocked_ports})}
    pub fn services(&self)->Vec<Service>{self.catalog.local_services().into_iter().filter(|s|PathBuf::from(&s.project_cwd)==self.checkout && !self.blocked_ports.contains(&s.port)).filter_map(|s|{let value=Service{id:s.id,name:s.name,port:s.port};value.validate().ok().map(|_|value)}).take(64).collect()}
}
#[async_trait::async_trait]
impl Connector for TaskConnector {
    async fn connect(&self,id:&str)->Result<BoxIo>{
        ensure!(self.services().iter().any(|s|s.id==id),"preview_service_unavailable");
        let route=self.catalog.local_route(id).ok_or_else(||anyhow::anyhow!("preview_service_unavailable"))?;
        let (pid,started)=(route.listener.pid,route.listener.started_at);
        ensure!(tokio::task::spawn_blocking(move||discovery::same_process(pid,started)).await?,"preview_process_changed");
        let port=route.listener.address.port();
        ensure!(!self.blocked_ports.contains(&port),"preview_service_port_reserved");
        // Never dial a runtime-supplied external/metadata address.
        let ip=if route.listener.address.is_ipv6(){IpAddr::V6(std::net::Ipv6Addr::LOCALHOST)}else{IpAddr::V4(Ipv4Addr::LOCALHOST)};
        Ok(Box::new(tokio::net::TcpStream::connect(SocketAddr::new(ip,port)).await?))
    }
}
