//! Minimal mDNS test using zeroconf-tokio.
//!
//! Run with: cargo run --example mdns_test

use zeroconf_tokio::prelude::*;
use zeroconf_tokio::{MdnsService, MdnsServiceAsync, ServiceType, TxtRecord};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("Creating mDNS service...");

    let service_type = ServiceType::new("qobuz-connect", "tcp")?;
    let mut service = MdnsService::new(service_type, 7864);

    service.set_name("QonductorTest");

    let mut txt = TxtRecord::new();
    txt.insert("path", "/streamcore")?;
    txt.insert("type", "SPEAKER")?;
    txt.insert("Name", "QonductorTest")?;
    txt.insert("device_uuid", "deadbeef12345678")?;
    service.set_txt_record(txt);

    println!("Starting service...");
    let mut service_async = MdnsServiceAsync::new(service)?;
    let registration = service_async.start().await?;

    println!("\nService registered!");
    println!("  Name: {}", registration.name());
    println!("  Domain: {}", registration.domain());
    println!("\nCheck with: avahi-browse -a --resolve -t | grep qobuz");
    println!("\nPress Ctrl+C to exit...");

    tokio::signal::ctrl_c().await?;
    drop(service_async);
    Ok(())
}
