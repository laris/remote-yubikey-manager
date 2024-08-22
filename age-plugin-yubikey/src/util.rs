use std::fmt;
use std::iter;

use x509_parser::{certificate::X509Certificate, der_parser::oid::Oid};
use yubikey::{
    piv::{RetiredSlotId, SlotId},
    Certificate, PinPolicy, Serial, TouchPolicy, YubiKey,
};

use crate::fl;
use crate::{error::Error, key::Stub, p256::Recipient, BINARY_NAME, USABLE_SLOTS};

pub(crate) const POLICY_EXTENSION_OID: &[u64] = &[1, 3, 6, 1, 4, 1, 41482, 3, 8];

pub(crate) fn ui_to_slot(slot: u8) -> Result<RetiredSlotId, Error> {
    // Use 1-indexing in the UI for niceness
    USABLE_SLOTS
        .get(slot as usize - 1)
        .cloned()
        .ok_or(Error::InvalidSlot(slot))
}

pub(crate) fn slot_to_ui(slot: &RetiredSlotId) -> u8 {
    // Use 1-indexing in the UI for niceness
    USABLE_SLOTS.iter().position(|s| s == slot).unwrap() as u8 + 1
}

pub(crate) fn pin_policy_from_string(s: String) -> Result<PinPolicy, Error> {
    match s.as_str() {
        "always" => Ok(PinPolicy::Always),
        "once" => Ok(PinPolicy::Once),
        "never" => Ok(PinPolicy::Never),
        _ => Err(Error::InvalidPinPolicy(s)),
    }
}

pub(crate) fn touch_policy_from_string(s: String) -> Result<TouchPolicy, Error> {
    match s.as_str() {
        "always" => Ok(TouchPolicy::Always),
        "cached" => Ok(TouchPolicy::Cached),
        "never" => Ok(TouchPolicy::Never),
        _ => Err(Error::InvalidTouchPolicy(s)),
    }
}

pub(crate) fn pin_policy_to_str(policy: Option<PinPolicy>) -> String {
    match policy {
        Some(PinPolicy::Always) => fl!("pin-policy-always"),
        Some(PinPolicy::Once) => fl!("pin-policy-once"),
        Some(PinPolicy::Never) => fl!("pin-policy-never"),
        _ => fl!("unknown-policy"),
    }
}

pub(crate) fn touch_policy_to_str(policy: Option<TouchPolicy>) -> String {
    match policy {
        Some(TouchPolicy::Always) => fl!("touch-policy-always"),
        Some(TouchPolicy::Cached) => fl!("touch-policy-cached"),
        Some(TouchPolicy::Never) => fl!("touch-policy-never"),
        _ => fl!("unknown-policy"),
    }
}

const MODHEX: &str = "cbdefghijklnrtuv";
pub(crate) fn otp_serial_prefix(serial: Serial) -> String {
    iter::repeat(0)
        .take(4)
        .chain((0..8).rev().map(|i| (serial.0 >> (4 * i)) & 0x0f))
        .map(|i| MODHEX.char_indices().nth(i as usize).unwrap().1)
        .collect()
}

pub(crate) fn extract_name(cert: &X509Certificate, all: bool) -> Option<(String, bool)> {
    // Look at Subject Organization to determine if we created this.
    match cert.subject().iter_organization().next() {
        Some(org) if org.as_str() == Ok(BINARY_NAME) => {
            // We store the identity name as a Common Name attribute.
            let name = cert
                .subject()
                .iter_common_name()
                .next()
                .and_then(|cn| cn.as_str().ok())
                .map(|s| s.to_owned())
                .unwrap_or_default(); // TODO: This should always be present.

            Some((name, true))
        }
        _ => {
            // Not one of ours, but we've already filtered for compatibility.
            if !all {
                return None;
            }

            // Display the entire subject.
            let name = cert.subject().to_string();

            Some((name, false))
        }
    }
}

pub(crate) struct Metadata {
    serial: Serial,
    slot: RetiredSlotId,
    name: String,
    created: String,
    pub(crate) pin_policy: Option<PinPolicy>,
    pub(crate) touch_policy: Option<TouchPolicy>,
}

impl Metadata {
    pub(crate) fn extract(
        yubikey: &mut YubiKey,
        slot: RetiredSlotId,
        cert: &Certificate,
        all: bool,
    ) -> Option<Self> {
        let (_, cert) = x509_parser::parse_x509_certificate(cert.as_ref()).ok()?;

        // We store the PIN and touch policies for identities in their certificates
        // using the same certificate extension as PIV attestations.
        // https://developers.yubico.com/PIV/Introduction/PIV_attestation.html
        let policies = |c: &X509Certificate| {
            c.tbs_certificate
                .get_extension_unique(&Oid::from(POLICY_EXTENSION_OID).unwrap())
                // If the extension is duplicated, we assume it is invalid.
                .ok()
                .flatten()
                // If the encoded extension doesn't have 2 bytes, we assume it is invalid.
                .filter(|policy| policy.value.len() >= 2)
                .map(|policy| {
                    // We should only ever see one of three values for either policy, but
                    // handle unknown values just in case.
                    let pin_policy = match policy.value[0] {
                        0x01 => Some(PinPolicy::Never),
                        0x02 => Some(PinPolicy::Once),
                        0x03 => Some(PinPolicy::Always),
                        _ => None,
                    };
                    let touch_policy = match policy.value[1] {
                        0x01 => Some(TouchPolicy::Never),
                        0x02 => Some(TouchPolicy::Always),
                        0x03 => Some(TouchPolicy::Cached),
                        _ => None,
                    };
                    (pin_policy, touch_policy)
                })
                .unwrap_or((None, None))
        };

        extract_name(&cert, all)
            .map(|(name, ours)| {
                if ours {
                    let (pin_policy, touch_policy) = policies(&cert);
                    (name, pin_policy, touch_policy)
                } else {
                    // We can extract the PIN and touch policies via an attestation. This
                    // is slow, but the user has asked for all compatible keys, so...
                    let (pin_policy, touch_policy) =
                        yubikey::piv::attest(yubikey, SlotId::Retired(slot))
                            .ok()
                            .and_then(|buf| {
                                x509_parser::parse_x509_certificate(&buf)
                                    .map(|(_, c)| policies(&c))
                                    .ok()
                            })
                            .unwrap_or((None, None));

                    (name, pin_policy, touch_policy)
                }
            })
            .map(|(name, pin_policy, touch_policy)| Metadata {
                serial: yubikey.serial(),
                slot,
                name,
                created: cert
                    .validity()
                    .not_before
                    .to_rfc2822()
                    .unwrap_or_else(|e| format!("Invalid date: {e}")),
                pin_policy,
                touch_policy,
            })
    }
}

impl fmt::Display for Metadata {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}",
            fl!(
                "yubikey-metadata",
                serial = self.serial.to_string(),
                slot = slot_to_ui(&self.slot),
                name = self.name.as_str(),
                created = self.created.as_str(),
                pin_policy = pin_policy_to_str(self.pin_policy),
                touch_policy = touch_policy_to_str(self.touch_policy),
            )
        )
    }
}

pub(crate) fn print_identity(stub: Stub, recipient: Recipient, metadata: Metadata) {
    let recipient = recipient.to_string();
    if !console::user_attended() {
        let recipient = recipient.as_str();
        eprintln!("{}", fl!("print-recipient", recipient = recipient));
    }

    println!(
        "{}",
        fl!(
            "yubikey-identity",
            yubikey_metadata = metadata.to_string(),
            recipient = recipient,
            identity = stub.to_string(),
        )
    );
}
