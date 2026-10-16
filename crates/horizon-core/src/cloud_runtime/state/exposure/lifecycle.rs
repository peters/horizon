//! Metadata for commands sent by the original held controller. No transition
//! restores authority or proves physical denial without the actual worker result.
use super::{Intent, MAX_LIFETIME_MILLIS, Phase, Record, Service, Startup};
use crate::cloud_runtime::state::Error;
use uuid::Uuid;

const INVALID: &str = "Exposure command does not match original live custody";

impl Record {
    fn same_custody(&self, epoch: Uuid, custody: Uuid) -> Result<(), Error> {
        if self.epoch != epoch || self.startup != (Startup::Observed { custody }) {
            return Err(Error::Invalid(INVALID));
        }
        Ok(())
    }

    fn fresh_operation(&self, operation: Uuid) -> Result<(), Error> {
        if operation.is_nil() || self.intents.iter().any(|intent| intent.operation == operation) {
            return Err(Error::Invalid(INVALID));
        }
        Ok(())
    }

    fn replace(&mut self, next: Self) -> Result<(), Error> {
        if !next.valid() {
            return Err(Error::Invalid(INVALID));
        }
        *self = next;
        Ok(())
    }

    /// Retain an additional service before its Open command. Services expire independently.
    /// # Errors
    /// Refuses stale custody, duplicate operations and clock rollback.
    pub fn add(
        &mut self,
        epoch: Uuid,
        custody: Uuid,
        service: Service,
        operation: Uuid,
        now: i64,
    ) -> Result<(), Error> {
        self.same_custody(epoch, custody)?;
        self.fresh_operation(operation)?;
        if now < self.observed_at_millis {
            return Err(Error::Invalid(INVALID));
        }
        let mut next = self.clone();
        next.intents.push(Intent {
            operation,
            service,
            started_at_millis: now,
            expires_at_millis: now.checked_add(MAX_LIFETIME_MILLIS).ok_or(Error::Invalid(INVALID))?,
            phase: Phase::Opening {},
        });
        next.observed_at_millis = now;
        self.replace(next)
    }

    /// Deliberate owner renewal, durably saved before its command. Retries retain this intent.
    /// # Errors
    /// Refuses unobserved/expired services, reused operations and rollback; siblings are unchanged.
    pub fn renewing(
        &mut self,
        epoch: Uuid,
        custody: Uuid,
        service: Uuid,
        operation: Uuid,
        now: i64,
    ) -> Result<(), Error> {
        self.same_custody(epoch, custody)?;
        self.fresh_operation(operation)?;
        if now < self.observed_at_millis {
            return Err(Error::Invalid(INVALID));
        }
        let mut next = self.clone();
        let intent = next
            .intents
            .iter_mut()
            .find(|intent| intent.service.id == service)
            .ok_or(Error::Invalid(INVALID))?;
        let Phase::Active { ingress } = intent.phase else {
            return Err(Error::Invalid(INVALID));
        };
        if now < intent.started_at_millis || now >= intent.expires_at_millis {
            return Err(Error::Invalid(INVALID));
        }
        intent.operation = operation;
        intent.started_at_millis = now;
        intent.expires_at_millis = now.checked_add(MAX_LIFETIME_MILLIS).ok_or(Error::Invalid(INVALID))?;
        intent.phase = Phase::Renewing { ingress };
        next.observed_at_millis = now;
        self.replace(next)
    }

    /// Retain a same-command worker gate observation. This is not public or TLS readiness.
    /// # Errors
    /// Refuses expired, stale, conflicting-port or backwards-clock observations.
    pub fn active(&mut self, epoch: Uuid, custody: Uuid, operation: Uuid, ingress: u16, now: i64) -> Result<(), Error> {
        self.same_custody(epoch, custody)?;
        if now < self.observed_at_millis {
            return Err(Error::Invalid(INVALID));
        }
        let mut next = self.clone();
        let intent = next
            .intents
            .iter_mut()
            .find(|intent| intent.operation == operation)
            .ok_or(Error::Invalid(INVALID))?;
        if !matches!(intent.phase, Phase::Opening {} | Phase::Renewing { .. })
            || intent.phase.ingress().is_some_and(|old| old != ingress)
            || now < intent.started_at_millis
            || now >= intent.expires_at_millis
        {
            return Err(Error::Invalid(INVALID));
        }
        intent.phase = Phase::Active { ingress };
        next.observed_at_millis = now;
        self.replace(next)
    }

    /// Save a fresh service close operation before dispatch. Expiry never prevents closing.
    /// # Errors
    /// Refuses another owner generation, stale command or already closing/closed service.
    pub fn closing(&mut self, epoch: Uuid, custody: Uuid, service: Uuid, operation: Uuid) -> Result<(), Error> {
        self.same_custody(epoch, custody)?;
        self.fresh_operation(operation)?;
        let mut next = self.clone();
        let intent = next
            .intents
            .iter_mut()
            .find(|intent| intent.service.id == service)
            .ok_or(Error::Invalid(INVALID))?;
        if matches!(intent.phase, Phase::Closing { .. } | Phase::Closed { .. }) {
            return Err(Error::Invalid(INVALID));
        }
        intent.operation = operation;
        intent.phase = Phase::Closing {
            ingress: intent.phase.ingress(),
        };
        self.replace(next)
    }

    /// Record denial confirmed by the exact service's original close command.
    /// # Errors
    /// Refuses stale Open/Renew responses, another service's port and unsolicited results.
    pub fn service_closed(&mut self, epoch: Uuid, custody: Uuid, operation: Uuid, ingress: u16) -> Result<(), Error> {
        self.same_custody(epoch, custody)?;
        let mut next = self.clone();
        let intent = next
            .intents
            .iter_mut()
            .find(|intent| intent.operation == operation)
            .ok_or(Error::Invalid(INVALID))?;
        let Phase::Closing { ingress: old } = intent.phase else {
            return Err(Error::Invalid(INVALID));
        };
        if !intent.service.route.accepts_ingress(ingress) || old.is_some_and(|old| old != ingress) {
            return Err(Error::Invalid(INVALID));
        }
        intent.phase = Phase::Closed { ingress: Some(ingress) };
        self.replace(next)
    }

    /// Save the original keeper's Finish command, including after uncertain observation.
    /// # Errors
    /// Unknown custody without an actual original handle cannot finish or adopt a successor.
    pub fn finishing(&mut self, epoch: Uuid, custody: Uuid, operation: Uuid) -> Result<(), Error> {
        if self.epoch != epoch
            || custody.is_nil()
            || !matches!(self.startup, Startup::Observed { custody: old } | Startup::Unknown { custody: Some(old) } if old == custody)
        {
            return Err(Error::Invalid(INVALID));
        }
        self.fresh_operation(operation)?;
        let mut next = self.clone();
        for intent in &mut next.intents {
            if !matches!(intent.phase, Phase::Closed { .. }) {
                intent.phase = Phase::Closing {
                    ingress: intent.phase.ingress(),
                };
            }
        }
        next.startup = Startup::Finishing { custody, operation };
        self.replace(next)
    }

    /// Metadata only: caller must confirm matching worker Finish and reap its owned child.
    /// # Errors
    /// Refuses every result except the original saved Finish operation.
    pub fn finished(&mut self, epoch: Uuid, custody: Uuid, operation: Uuid) -> Result<(), Error> {
        if self.epoch != epoch
            || !matches!(self.startup, Startup::Finishing { custody: original, operation: command } | Startup::FinishUnknown { custody: original, operation: command } if original == custody && command == operation)
        {
            return Err(Error::Invalid(INVALID));
        }
        let mut next = self.clone();
        for intent in &mut next.intents {
            intent.phase = Phase::Closed {
                ingress: intent.phase.ingress(),
            };
        }
        next.startup = Startup::Closed { custody };
        self.replace(next)
    }
}
