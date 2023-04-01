const MSG_RX_STATE_BITMAP_LEN: u32 = 16;

#[derive(Debug)]
pub struct RxCtrState {
    max_ctr: u32,
    ctr_bitmap: u16,
}

impl RxCtrState {
    pub fn new(max_ctr: u32) -> Self {
        Self {
            max_ctr,
            ctr_bitmap: 0xffff,
        }
    }

    fn contains(&self, bit_number: u32) -> bool {
        (self.ctr_bitmap & (1 << bit_number)) != 0
    }

    fn insert(&mut self, bit_number: u32) {
        self.ctr_bitmap |= 1 << bit_number;
    }

    /// Receive a message and update Rx State accordingly
    /// Returns a bool indicating whether the message is a duplicate
    pub fn recv(&mut self, msg_ctr: u32, is_encrypted: bool) -> bool {
        let idiff = (msg_ctr as i32) - (self.max_ctr as i32);
        let udiff = idiff.unsigned_abs();

        if msg_ctr == self.max_ctr {
            // Duplicate
            true
        } else if (-(MSG_RX_STATE_BITMAP_LEN as i32)..0).contains(&idiff) {
            // In Rx Bitmap
            let index = udiff - 1;
            if self.contains(index) {
                // Duplicate
                true
            } else {
                self.insert(index);
                false
            }
        }
        // Now the leftover cases are the new counter is outside of the bitmap as well as max_ctr
        // in either direction. Encrypted only allows in forward direction
        else if msg_ctr > self.max_ctr {
            self.max_ctr = msg_ctr;
            if udiff < MSG_RX_STATE_BITMAP_LEN {
                // The previous max_ctr is now the actual counter
                self.ctr_bitmap <<= udiff;
                self.insert(udiff - 1);
            } else {
                self.ctr_bitmap = 0xffff;
            }
            false
        } else if !is_encrypted {
            // This is the case where the peer possibly rebooted and chose a different
            // random counter
            self.max_ctr = msg_ctr;
            self.ctr_bitmap = 0xffff;
            false
        } else {
            true
        }
    }
}

#[cfg(test)]
mod tests {

    use super::RxCtrState;

    const ENCRYPTED: bool = true;
    const NOT_ENCRYPTED: bool = false;

    fn assert_ndup(b: bool) {
        assert!(!b);
    }

    fn assert_dup(b: bool) {
        assert!(b);
    }

    #[test]
    fn new_msg_ctr() {
        let mut s = RxCtrState::new(101);

        assert_ndup(s.recv(103, ENCRYPTED));
        assert_ndup(s.recv(104, ENCRYPTED));
        assert_ndup(s.recv(106, ENCRYPTED));
        assert_eq!(s.max_ctr, 106);
        assert_eq!(s.ctr_bitmap, 0b1111_1111_1111_0110);

        assert_ndup(s.recv(118, NOT_ENCRYPTED));
        assert_eq!(s.ctr_bitmap, 0b0110_1000_0000_0000);
        assert_ndup(s.recv(119, NOT_ENCRYPTED));
        assert_ndup(s.recv(121, NOT_ENCRYPTED));
        assert_eq!(s.ctr_bitmap, 0b0100_0000_0000_0110);
    }

    #[test]
    fn dup_max_ctr() {
        let mut s = RxCtrState::new(101);

        assert_ndup(s.recv(103, ENCRYPTED));
        assert_dup(s.recv(103, ENCRYPTED));
        assert_dup(s.recv(103, NOT_ENCRYPTED));

        assert_eq!(s.max_ctr, 103);
        assert_eq!(s.ctr_bitmap, 0b1111_1111_1111_1110);
    }

    #[test]
    fn dup_in_rx_bitmap() {
        let mut ctr = 101;
        let mut s = RxCtrState::new(101);
        for _ in 1..8 {
            ctr += 2;
            assert_ndup(s.recv(ctr, ENCRYPTED));
        }
        assert_ndup(s.recv(116, ENCRYPTED));
        assert_ndup(s.recv(117, ENCRYPTED));
        assert_eq!(s.max_ctr, 117);
        assert_eq!(s.ctr_bitmap, 0b1010_1010_1010_1011);

        // duplicate on the left corner
        assert_dup(s.recv(101, ENCRYPTED));
        assert_dup(s.recv(101, NOT_ENCRYPTED));

        // duplicate on the right corner
        assert_dup(s.recv(116, ENCRYPTED));
        assert_dup(s.recv(116, NOT_ENCRYPTED));

        // valid insert
        assert_ndup(s.recv(102, ENCRYPTED));
        assert_dup(s.recv(102, ENCRYPTED));
        assert_eq!(s.ctr_bitmap, 0b1110_1010_1010_1011);
    }

    #[test]
    fn valid_corners_in_rx_bitmap() {
        let mut ctr = 102;
        let mut s = RxCtrState::new(101);
        for _ in 1..9 {
            ctr += 2;
            assert_ndup(s.recv(ctr, ENCRYPTED));
        }
        assert_eq!(s.max_ctr, 118);
        assert_eq!(s.ctr_bitmap, 0b0010_1010_1010_1010);

        // valid insert on the left corner
        assert_ndup(s.recv(102, ENCRYPTED));
        assert_eq!(s.ctr_bitmap, 0b1010_1010_1010_1010);

        // valid insert on the right corner
        assert_ndup(s.recv(117, ENCRYPTED));
        assert_eq!(s.ctr_bitmap, 0b1010_1010_1010_1011);
    }

    #[test]
    fn encrypted_wraparound() {
        let mut s = RxCtrState::new(65534);

        assert_ndup(s.recv(65535, ENCRYPTED));
        assert_ndup(s.recv(65536, ENCRYPTED));
        assert_dup(s.recv(0, ENCRYPTED));
    }

    #[test]
    fn unencrypted_wraparound() {
        let mut s = RxCtrState::new(65534);

        assert_ndup(s.recv(65536, NOT_ENCRYPTED));
        assert_ndup(s.recv(0, NOT_ENCRYPTED));
    }

    #[test]
    fn unencrypted_device_reboot() {
        println!("Sub 65532 is {:?}", 1_u16.overflowing_sub(65532));
        println!("Sub 65535 is {:?}", 1_u16.overflowing_sub(65535));
        println!("Sub 11-13 is {:?}", 11_u32.wrapping_sub(13_u32) as i32);
        println!("Sub regular is {:?}", 2000_u16.overflowing_sub(1998));
        let mut s = RxCtrState::new(20010);

        assert_ndup(s.recv(20011, NOT_ENCRYPTED));
        assert_ndup(s.recv(0, NOT_ENCRYPTED));
    }
}
