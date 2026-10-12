use std::time::Duration;

use ureq::Error;
use ureq::Timeout;
use ureq::unversioned::transport::{Buffers, ConnectionDetails, Connector, NextTimeout, Transport};

#[derive(Debug, Clone, Copy)]
pub(crate) struct BodyPoll(pub(crate) Duration);

impl<In: Transport> Connector<In> for BodyPoll {
    type Out = Polled<In>;

    fn connect(
        &self,
        _details: &ConnectionDetails,
        chained: Option<In>,
    ) -> Result<Option<Self::Out>, Error> {
        Ok(chained.map(|inner| Polled {
            inner,
            every: self.0,
        }))
    }
}

#[derive(Debug)]
pub(crate) struct Polled<T> {
    inner: T,
    every: Duration,
}

impl<T: Transport> Transport for Polled<T> {
    fn buffers(&mut self) -> &mut dyn Buffers {
        self.inner.buffers()
    }

    fn transmit_output(&mut self, amount: usize, timeout: NextTimeout) -> Result<(), Error> {
        self.inner.transmit_output(amount, timeout)
    }

    fn await_input(&mut self, mut timeout: NextTimeout) -> Result<bool, Error> {
        if timeout.reason == Timeout::RecvBody && *timeout.after > self.every {
            timeout.after = self.every.into();
        }
        self.inner.await_input(timeout)
    }

    fn is_open(&mut self) -> bool {
        self.inner.is_open()
    }

    fn is_tls(&self) -> bool {
        self.inner.is_tls()
    }
}
