use dialoguer::Password;
use rand::{rngs::OsRng, RngCore};
use x509::RelativeDistinguishedName;
use yubikey::{
    certificate::Certificate,
    piv::{generate as yubikey_generate, AlgorithmId, RetiredSlotId, SlotId},
    Key, PinPolicy, TouchPolicy, YubiKey,
};

use crate::{
    error::Error,
    fl,
    key::{self, Stub},
    p256::Recipient,
    util::{Metadata, POLICY_EXTENSION_OID},
    BINARY_NAME, USABLE_SLOTS,
};

pub(crate) const DEFAULT_PIN_POLICY: PinPolicy = PinPolicy::Once;
pub(crate) const DEFAULT_TOUCH_POLICY: TouchPolicy = TouchPolicy::Always;

pub(crate) struct IdentityBuilder {
    slot: Option<RetiredSlotId>,
    force: bool,
    name: Option<String>,
    pin_policy: Option<PinPolicy>,
    touch_policy: Option<TouchPolicy>,
}

impl IdentityBuilder {
    pub(crate) fn new(slot: Option<RetiredSlotId>) -> Self {
        IdentityBuilder {
            slot,
            name: None,
            pin_policy: None,
            touch_policy: None,
            force: false,
        }
    }

    pub(crate) fn with_name(mut self, name: Option<String>) -> Self {
        self.name = name;
        self
    }

    pub(crate) fn with_pin_policy(mut self, pin_policy: Option<PinPolicy>) -> Self {
        self.pin_policy = pin_policy;
        self
    }

    pub(crate) fn with_touch_policy(mut self, touch_policy: Option<TouchPolicy>) -> Self {
        self.touch_policy = touch_policy;
        self
    }

    pub(crate) fn force(mut self, force: bool) -> Self {
        self.force = force;
        self
    }

    pub(crate) fn build(self, yubikey: &mut YubiKey) -> Result<(Stub, Recipient, Metadata), Error> {
        let slot = match self.slot {
            Some(slot) => {
                if !self.force {
                    // Check that the slot is empty.
                    if Key::list(yubikey)?
                        .into_iter()
                        .any(|key| key.slot() == SlotId::Retired(slot))
                    {
                        return Err(Error::SlotIsNotEmpty(slot));
                    }
                }

                // Now either the slot is empty, or --force is specified.
                slot
            }
            None => {
                // Use the first empty slot.
                let keys = Key::list(yubikey)?;
                USABLE_SLOTS
                    .iter()
                    .find(|&&slot| !keys.iter().any(|key| key.slot() == SlotId::Retired(slot)))
                    .cloned()
                    .ok_or_else(|| Error::NoEmptySlots(yubikey.serial()))?
            }
        };

        let pin_policy = self.pin_policy.unwrap_or(DEFAULT_PIN_POLICY);
        let touch_policy = self.touch_policy.unwrap_or(DEFAULT_TOUCH_POLICY);

        eprintln!("{}", fl!("builder-gen-key"));

        // No need to ask for users to enter their PIN if the PIN policy requires it,
        // because here we _always_ require them to enter their PIN in order to access the
        // protected management key (which is necessary in order to generate identities).
        key::manage(yubikey)?;

        // Generate a new key in the selected slot.
        let generated = yubikey_generate(
            yubikey,
            SlotId::Retired(slot),
            AlgorithmId::EccP256,
            pin_policy,
            touch_policy,
        )?;

        let recipient = Recipient::from_spki(&generated).expect("YubiKey generates a valid pubkey");
        let stub = Stub::new(yubikey.serial(), slot, &recipient);

        eprintln!();
        eprintln!("{}", fl!("builder-gen-cert"));

        // Pick a random serial for the new self-signed certificate.
        let mut serial = [0; 20];
        OsRng.fill_bytes(&mut serial);

        let name = self
            .name
            .unwrap_or(format!("age identity {}", hex::encode(stub.tag)));

        if let PinPolicy::Always = pin_policy {
            // We need to enter the PIN again.
            let pin = Password::new()
                .with_prompt(fl!(
                    "plugin-enter-pin",
                    yubikey_serial = yubikey.serial().to_string(),
                ))
                .report(true)
                .interact()?;
            yubikey.verify_pin(pin.as_bytes())?;
        }
        if let TouchPolicy::Never = touch_policy {
            // No need to touch YubiKey
        } else {
            eprintln!("{}", fl!("builder-touch-yk"));
        }

        let cert = Certificate::generate_self_signed(
            yubikey,
            SlotId::Retired(slot),
            serial,
            None,
            &[
                RelativeDistinguishedName::organization(BINARY_NAME),
                RelativeDistinguishedName::organizational_unit(env!("CARGO_PKG_VERSION")),
                RelativeDistinguishedName::common_name(&name),
            ],
            generated,
            &[x509::Extension::regular(
                POLICY_EXTENSION_OID,
                &[pin_policy.into(), touch_policy.into()],
            )],
        )?;

        let metadata = Metadata::extract(yubikey, slot, &cert, false).unwrap();

        Ok((
            Stub::new(yubikey.serial(), slot, &recipient),
            recipient,
            metadata,
        ))
    }
}
