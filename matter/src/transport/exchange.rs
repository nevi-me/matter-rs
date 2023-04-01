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

use boxslab::{BoxSlab, Slab};
use colored::*;
use log::{error, info, trace};
use std::any::Any;
use std::fmt;
use std::time::SystemTime;

use crate::error::Error;
use crate::secure_channel;

use heapless::LinearMap;

use super::packet::PacketPool;
use super::session::CloneData;
use super::{mrp::ReliableMessage, packet::Packet, session::SessionHandle, session::SessionMgr};

pub struct ExchangeCtx<'a> {
    pub exch: &'a mut Exchange,
    pub sess: SessionHandle<'a>,
}

#[derive(Debug, PartialEq, Eq, Copy, Clone)]
pub enum Role {
    Initiator = 0,
    Responder = 1,
}

impl Default for Role {
    fn default() -> Self {
        Role::Initiator
    }
}

/// State of the exchange
#[derive(Debug, PartialEq)]
enum State {
    /// The exchange is open and active
    Open,
    /// The exchange is closed, but keys are active since retransmissions/acks may be pending
    Close,
    /// The exchange is terminated, keys are destroyed, no communication can happen
    Terminate,
}

impl Default for State {
    fn default() -> Self {
        State::Open
    }
}

// Instead of just doing an Option<>, we create some special handling
// where the commonly used higher layer data store does't have to do a Box
#[derive(Debug)]
pub enum DataOption {
    Boxed(Box<dyn Any>),
    Time(SystemTime),
    None,
}

impl Default for DataOption {
    fn default() -> Self {
        DataOption::None
    }
}

#[derive(Debug, Default)]
pub struct Exchange {
    id: u16,
    sess_idx: usize,
    role: Role,
    state: State,
    mrp: ReliableMessage,
    // Currently I see this primarily used in PASE and CASE. If that is the limited use
    // of this, we might move this into a separate data structure, so as not to burden
    // all 'exchanges'.
    data: DataOption,
}

impl Exchange {
    pub fn new(id: u16, sess_idx: usize, role: Role) -> Exchange {
        Exchange {
            id,
            sess_idx,
            role,
            state: State::Open,
            mrp: ReliableMessage::new(),
            ..Default::default()
        }
    }

    pub fn terminate(&mut self) {
        self.data = DataOption::None;
        self.state = State::Terminate;
    }

    pub fn close(&mut self) {
        self.data = DataOption::None;
        self.state = State::Close;
    }

    pub fn is_state_open(&self) -> bool {
        self.state == State::Open
    }

    pub fn is_purgeable(&self) -> bool {
        // No Users, No pending ACKs/Retrans
        self.state == State::Terminate || (self.state == State::Close && self.mrp.is_empty())
    }

    pub fn get_id(&self) -> u16 {
        self.id
    }

    pub fn get_role(&self) -> Role {
        self.role
    }

    pub fn is_data_none(&self) -> bool {
        matches!(self.data, DataOption::None)
    }

    pub fn set_data_boxed(&mut self, data: Box<dyn Any>) {
        self.data = DataOption::Boxed(data);
    }

    pub fn clear_data_boxed(&mut self) {
        self.data = DataOption::None;
    }

    pub fn get_data_boxed<T: Any>(&mut self) -> Option<&mut T> {
        if let DataOption::Boxed(a) = &mut self.data {
            a.downcast_mut::<T>()
        } else {
            None
        }
    }

    pub fn take_data_boxed<T: Any>(&mut self) -> Option<Box<T>> {
        let old = std::mem::replace(&mut self.data, DataOption::None);
        if let DataOption::Boxed(d) = old {
            d.downcast::<T>().ok()
        } else {
            self.data = old;
            None
        }
    }

    pub fn set_data_time(&mut self, expiry_ts: Option<SystemTime>) {
        if let Some(t) = expiry_ts {
            self.data = DataOption::Time(t);
        }
    }

    pub fn get_data_time(&self) -> Option<SystemTime> {
        match self.data {
            DataOption::Time(t) => Some(t),
            _ => None,
        }
    }

    pub fn send(
        &mut self,
        mut proto_tx: BoxSlab<PacketPool>,
        session: &mut SessionHandle,
    ) -> Result<(), Error> {
        if self.state == State::Terminate {
            info!("Skipping tx for terminated exchange {}", self.id);
            return Ok(());
        }

        trace!("payload: {:x?}", proto_tx.as_borrow_slice());
        info!(
            "{} with proto id: {} opcode: {}",
            "Sending".blue(),
            proto_tx.get_proto_id(),
            proto_tx.get_proto_opcode(),
        );

        proto_tx.proto.exch_id = self.id;
        if self.role == Role::Initiator {
            proto_tx.proto.set_initiator();
        }

        session.pre_send(&mut proto_tx)?;
        self.mrp.pre_send(&mut proto_tx)?;
        session.send(proto_tx)
    }
}

impl fmt::Display for Exchange {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "exch_id: {:?}, sess_index: {}, role: {:?}, data: {:?}, mrp: {:?}, state: {:?}",
            self.id, self.sess_idx, self.role, self.data, self.mrp, self.state
        )
    }
}

pub fn get_role(is_initiator: bool) -> Role {
    if is_initiator {
        Role::Initiator
    } else {
        Role::Responder
    }
}

pub fn get_complementary_role(is_initiator: bool) -> Role {
    if is_initiator {
        Role::Responder
    } else {
        Role::Initiator
    }
}

const MAX_EXCHANGES: usize = 8;

#[derive(Default)]
pub struct ExchangeMgr {
    // keys: exch-id
    exchanges: LinearMap<u16, Exchange, MAX_EXCHANGES>,
    sess_mgr: SessionMgr,
}

pub const MAX_MRP_ENTRIES: usize = 4;

impl ExchangeMgr {
    pub fn new(sess_mgr: SessionMgr) -> Self {
        Self {
            sess_mgr,
            exchanges: Default::default(),
        }
    }

    pub fn get_sess_mgr(&mut self) -> &mut SessionMgr {
        &mut self.sess_mgr
    }

    pub fn _get_with_id(
        exchanges: &mut LinearMap<u16, Exchange, MAX_EXCHANGES>,
        exch_id: u16,
    ) -> Option<&mut Exchange> {
        exchanges.get_mut(&exch_id)
    }

    pub fn get_with_id(&mut self, exch_id: u16) -> Option<&mut Exchange> {
        ExchangeMgr::_get_with_id(&mut self.exchanges, exch_id)
    }

    fn _get(
        exchanges: &mut LinearMap<u16, Exchange, MAX_EXCHANGES>,
        sess_idx: usize,
        id: u16,
        role: Role,
        create_new: bool,
    ) -> Result<&mut Exchange, Error> {
        // I don't prefer that we scan the list twice here (once for contains_key and other)
        if !exchanges.contains_key(&(id)) {
            if create_new {
                // If an exchange doesn't exist, create a new one
                info!("Creating new exchange");
                let e = Exchange::new(id, sess_idx, role);
                if exchanges.insert(id, e).is_err() {
                    return Err(Error::NoSpace);
                }
            } else {
                return Err(Error::NoSpace);
            }
        }

        // At this point, we would either have inserted the record if 'create_new' was set
        // or it existed already
        if let Some(result) = exchanges.get_mut(&id) {
            if result.get_role() == role && sess_idx == result.sess_idx {
                Ok(result)
            } else {
                Err(Error::NoExchange)
            }
        } else {
            error!("This should never happen");
            Err(Error::NoSpace)
        }
    }

    /// The Exchange Mgr receive is like a big processing function
    pub fn recv(&mut self) -> Result<Option<(BoxSlab<PacketPool>, ExchangeCtx)>, Error> {
        // Get the session
        let (mut proto_rx, index) = self.sess_mgr.recv()?;

        let index = if let Some(s) = index {
            s
        } else {
            // The sessions were full, evict one session, and re-perform post-recv
            let evict_index = self.sess_mgr.get_lru();
            self.evict_session(evict_index)?;
            info!("Reattempting session creation");
            self.sess_mgr.post_recv(&proto_rx)?.ok_or(Error::Invalid)?
        };
        let mut session = self.sess_mgr.get_session_handle(index);

        // Decrypt the message
        session.recv(&mut proto_rx)?;

        // Get the exchange
        let exch = ExchangeMgr::_get(
            &mut self.exchanges,
            index,
            proto_rx.proto.exch_id,
            get_complementary_role(proto_rx.proto.is_initiator()),
            // We create a new exchange, only if the peer is the initiator
            proto_rx.proto.is_initiator(),
        )?;

        // Message Reliability Protocol
        exch.mrp.recv(&proto_rx)?;

        if exch.is_state_open() {
            Ok(Some((
                proto_rx,
                ExchangeCtx {
                    exch,
                    sess: session,
                },
            )))
        } else {
            // Instead of an error, we send None here, because it is likely that
            // we just processed an acknowledgement that cleared the exchange
            Ok(None)
        }
    }

    pub fn send(&mut self, exch_id: u16, proto_tx: BoxSlab<PacketPool>) -> Result<(), Error> {
        let exchange =
            ExchangeMgr::_get_with_id(&mut self.exchanges, exch_id).ok_or(Error::NoExchange)?;
        let mut session = self.sess_mgr.get_session_handle(exchange.sess_idx);
        exchange.send(proto_tx, &mut session)
    }

    pub fn purge(&mut self) {
        let mut to_purge: LinearMap<u16, (), MAX_EXCHANGES> = LinearMap::new();

        for (exch_id, exchange) in self.exchanges.iter() {
            if exchange.is_purgeable() {
                let _ = to_purge.insert(*exch_id, ());
            }
        }
        for (exch_id, _) in to_purge.iter() {
            self.exchanges.remove(exch_id);
        }
    }

    pub fn pending_acks(&mut self, expired_entries: &mut LinearMap<u16, (), MAX_MRP_ENTRIES>) {
        for (exch_id, exchange) in self.exchanges.iter() {
            if exchange.mrp.is_ack_ready() {
                expired_entries.insert(*exch_id, ()).unwrap();
            }
        }
    }

    pub fn evict_session(&mut self, index: usize) -> Result<(), Error> {
        info!("Sessions full, vacating session with index: {}", index);
        // If we enter here, we have an LRU session that needs to be reclaimed
        // As per the spec, we need to send a CLOSE here

        let mut session = self.sess_mgr.get_session_handle(index);
        let mut tx = Slab::<PacketPool>::try_new(Packet::new_tx()?).ok_or(Error::NoSpace)?;
        secure_channel::common::create_sc_status_report(
            &mut tx,
            secure_channel::common::SCStatusCodes::CloseSession,
            None,
        )?;

        if let Some((_, exchange)) = self.exchanges.iter_mut().find(|(_, e)| e.sess_idx == index) {
            // Send Close_session on this exchange, and then close the session
            // Should this be done for all exchanges?
            error!("Sending Close Session");
            exchange.send(tx, &mut session)?;
            // TODO: This wouldn't actually send it out, because 'transport' isn't owned yet.
        }

        let remove_exchanges: Vec<u16> = self
            .exchanges
            .iter()
            .filter_map(|(eid, e)| {
                if e.sess_idx == index {
                    Some(*eid)
                } else {
                    None
                }
            })
            .collect();
        info!(
            "Terminating the following exchanges: {:?}",
            remove_exchanges
        );
        for exch_id in remove_exchanges {
            // Remove from exchange list
            self.exchanges.remove(&exch_id);
        }
        self.sess_mgr.remove(index);
        Ok(())
    }

    pub fn add_session(&mut self, clone_data: &CloneData) -> Result<SessionHandle, Error> {
        let sess_idx = match self.sess_mgr.clone_session(clone_data) {
            Ok(idx) => idx,
            Err(Error::NoSpace) => {
                let evict_index = self.sess_mgr.get_lru();
                self.evict_session(evict_index)?;
                self.sess_mgr.clone_session(clone_data)?
            }
            Err(e) => {
                return Err(e);
            }
        };
        Ok(self.sess_mgr.get_session_handle(sess_idx))
    }
}

impl fmt::Display for ExchangeMgr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "{{  Session Mgr: {},", self.sess_mgr)?;
        writeln!(f, "  Exchanges: [")?;
        for s in &self.exchanges {
            writeln!(f, "{{ {}, }},", s.1)?;
        }
        writeln!(f, "  ]")?;
        write!(f, "}}")
    }
}

#[cfg(test)]
#[allow(clippy::bool_assert_comparison)]
mod tests {

    use crate::{
        error::Error,
        transport::{
            network::{Address, NetworkInterface},
            session::{CloneData, SessionMgr, SessionMode, MAX_SESSIONS},
        },
    };

    use super::{ExchangeMgr, Role};

    #[test]
    fn test_purge() {
        let sess_mgr = SessionMgr::new();
        let mut mgr = ExchangeMgr::new(sess_mgr);
        let _ = ExchangeMgr::_get(&mut mgr.exchanges, 1, 2, Role::Responder, true).unwrap();
        let _ = ExchangeMgr::_get(&mut mgr.exchanges, 1, 3, Role::Responder, true).unwrap();

        mgr.purge();
        assert_eq!(
            ExchangeMgr::_get(&mut mgr.exchanges, 1, 2, Role::Responder, false).is_ok(),
            true
        );
        assert_eq!(
            ExchangeMgr::_get(&mut mgr.exchanges, 1, 3, Role::Responder, false).is_ok(),
            true
        );

        // Close e1
        let e1 = ExchangeMgr::_get(&mut mgr.exchanges, 1, 2, Role::Responder, false).unwrap();
        e1.close();
        mgr.purge();
        assert_eq!(
            ExchangeMgr::_get(&mut mgr.exchanges, 1, 2, Role::Responder, false).is_ok(),
            false
        );
        assert_eq!(
            ExchangeMgr::_get(&mut mgr.exchanges, 1, 3, Role::Responder, false).is_ok(),
            true
        );
    }

    fn get_clone_data(peer_sess_id: u16, local_sess_id: u16) -> CloneData {
        CloneData::new(
            12341234,
            43211234,
            peer_sess_id,
            local_sess_id,
            Address::default(),
            SessionMode::Pase,
        )
    }

    fn fill_sessions(mgr: &mut ExchangeMgr, count: usize) {
        let mut local_sess_id = 1;
        let mut peer_sess_id = 100;
        for _ in 1..count {
            let clone_data = get_clone_data(peer_sess_id, local_sess_id);
            match mgr.add_session(&clone_data) {
                Ok(s) => assert_eq!(peer_sess_id, s.get_peer_sess_id()),
                Err(Error::NoSpace) => break,
                _ => {
                    panic!("Couldn't, create session");
                }
            }
            local_sess_id += 1;
            peer_sess_id += 1;
        }
    }

    pub struct DummyNetwork;
    impl DummyNetwork {
        pub fn new() -> Self {
            Self {}
        }
    }

    impl NetworkInterface for DummyNetwork {
        fn recv(&self, _in_buf: &mut [u8]) -> Result<(usize, Address), Error> {
            Ok((0, Address::default()))
        }

        fn send(&self, _out_buf: &[u8], _addr: Address) -> Result<usize, Error> {
            Ok(0)
        }
    }

    #[test]
    /// We purposefuly overflow the sessions
    /// and when the overflow happens, we confirm that
    /// - The sessions are evicted in LRU
    /// - The exchanges associated with those sessions are evicted too
    fn test_sess_evict() {
        let mut sess_mgr = SessionMgr::new();
        let transport = Box::new(DummyNetwork::new());
        sess_mgr.add_network_interface(transport).unwrap();
        let mut mgr = ExchangeMgr::new(sess_mgr);

        fill_sessions(&mut mgr, MAX_SESSIONS + 1);
        // Sessions are now full from local session id 1 to 16

        // Create exchanges for sessions 2 (i.e. session index 1) and 3 (session index 2)
        //   Exchange IDs are 20 and 30 respectively
        let _ = ExchangeMgr::_get(&mut mgr.exchanges, 1, 20, Role::Responder, true).unwrap();
        let _ = ExchangeMgr::_get(&mut mgr.exchanges, 2, 30, Role::Responder, true).unwrap();

        // Confirm that session ids 1 to MAX_SESSIONS exists
        for i in 1..(MAX_SESSIONS + 1) {
            assert_eq!(mgr.sess_mgr.get_with_id(i as u16).is_none(), false);
        }
        // Confirm that the exchanges are around
        assert_eq!(mgr.get_with_id(20).is_none(), false);
        assert_eq!(mgr.get_with_id(30).is_none(), false);
        let mut old_local_sess_id = 1;
        let mut new_local_sess_id = 100;
        let mut new_peer_sess_id = 200;

        for i in 1..(MAX_SESSIONS + 1) {
            // Now purposefully overflow the sessions by adding another session
            let session = mgr
                .add_session(&get_clone_data(new_peer_sess_id, new_local_sess_id))
                .unwrap();
            assert_eq!(session.get_peer_sess_id(), new_peer_sess_id);

            // This should have evicted session with local sess_id
            assert_eq!(mgr.sess_mgr.get_with_id(old_local_sess_id).is_none(), true);

            new_local_sess_id += 1;
            new_peer_sess_id += 1;
            old_local_sess_id += 1;

            match i {
                1 => {
                    // Both exchanges should exist
                    assert_eq!(mgr.get_with_id(20).is_none(), false);
                    assert_eq!(mgr.get_with_id(30).is_none(), false);
                }
                2 => {
                    // Exchange 20 would have been evicted
                    assert_eq!(mgr.get_with_id(20).is_none(), true);
                    assert_eq!(mgr.get_with_id(30).is_none(), false);
                }
                3 => {
                    // Exchange 20 and 30 would have been evicted
                    assert_eq!(mgr.get_with_id(20).is_none(), true);
                    assert_eq!(mgr.get_with_id(30).is_none(), true);
                }
                _ => {}
            }
        }
        //        println!("Session mgr {}", mgr.sess_mgr);
    }
}
