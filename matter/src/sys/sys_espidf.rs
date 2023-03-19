/*
 *
 *    Copyright (c) 2020-2022 Project CHIP Authors
 *
 *    Licensed under the Apache License, Version 2.0 (the "License");
 *    you may not use this file except in compliance with the License.
 *    You may obtain a copy of the License at
 *
 *        http://www.apache.org/licenses/LICENSE-2.0
 *
 *    Unless required by applicable law or agreed to in writing, software
 *    distributed under the License is distributed on an "AS IS" BASIS,
 *    WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 *    See the License for the specific language governing permissions and
 *    limitations under the License.
 */

use std::vec::Vec;

use crate::error::Error;
use esp_idf_svc::mdns::EspMdns;
use log::{error, info};

#[allow(dead_code)]
pub struct SysMdnsService {
    service: EspMdns,
}

/// Publish a mDNS service
/// name - can be a service name (comma separate subtypes may follow)
/// regtype - registration type (e.g. _matter_.tcp etc)
/// port - the port
pub fn sys_publish_service(
    name: &str,
    regtype: &str,
    port: u16,
    txt_kvs: &[[&str; 2]],
) -> Result<SysMdnsService, Error> {
    info!("mDNS Registration Type {}", regtype);
    info!("mDNS properties {:?}", txt_kvs);

    let kvs = txt_kvs
        .iter()
        .map(|kvs| (kvs[0], kvs[1]))
        .collect::<Vec<_>>();

    let mut service = EspMdns::take().map_err(|e| {
        error!("Error taking EspMdns service {:?}", e);
        Error::MdnsError
    })?;
    // TODO: mdns fails if this is not set, not sure of what value to use
    service.set_hostname(format!("local")).unwrap();
    // service.set_instance_name("instance_name").unwrap(); // not sure what to set here
    // TODO: a temporary hack, refactor the function
    let proto = if regtype.contains("_udp") {
        "_udp"
    } else {
        "_tcp"
    };
    service
        .add_service(
            Some(name),
            regtype.split('.').next().unwrap(),
            proto,
            port,
            &kvs,
        )
        .map_err(|e| {
            error!("Error adding EspMdns service {:?}", e);
            Error::MdnsError
        })?;

    Ok(SysMdnsService { service })
}
