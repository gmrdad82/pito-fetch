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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};
    use ureq::unversioned::transport::LazyBuffers;

    #[derive(Debug, Default)]
    struct Seen {
        awaited: Vec<NextTimeout>,
        transmitted: Vec<(usize, NextTimeout)>,
    }

    #[derive(Debug)]
    struct Fake {
        buffers: LazyBuffers,
        seen: Arc<Mutex<Seen>>,
        open: bool,
        tls: bool,
    }

    impl Transport for Fake {
        fn buffers(&mut self) -> &mut dyn Buffers {
            &mut self.buffers
        }

        fn transmit_output(&mut self, amount: usize, timeout: NextTimeout) -> Result<(), Error> {
            self.seen
                .lock()
                .unwrap()
                .transmitted
                .push((amount, timeout));
            if amount == 0 {
                return Err(Error::ConnectionFailed);
            }
            Ok(())
        }

        fn await_input(&mut self, timeout: NextTimeout) -> Result<bool, Error> {
            self.seen.lock().unwrap().awaited.push(timeout);
            Ok(timeout.reason == Timeout::RecvBody)
        }

        fn is_open(&mut self) -> bool {
            self.open
        }

        fn is_tls(&self) -> bool {
            self.tls
        }
    }

    const EVERY: Duration = Duration::from_millis(200);

    fn polled(open: bool, tls: bool) -> (Polled<Fake>, Arc<Mutex<Seen>>) {
        let seen = Arc::new(Mutex::new(Seen::default()));
        let fake = Fake {
            buffers: LazyBuffers::new(64, 64),
            seen: Arc::clone(&seen),
            open,
            tls,
        };
        (
            Polled {
                inner: fake,
                every: EVERY,
            },
            seen,
        )
    }

    fn timeout(reason: Timeout, after: Duration) -> NextTimeout {
        NextTimeout {
            after: after.into(),
            reason,
        }
    }

    fn awaited_with(input: NextTimeout) -> (bool, NextTimeout) {
        let (mut transport, seen) = polled(true, false);
        let progressed = transport.await_input(input).unwrap();
        let seen = seen.lock().unwrap();
        assert_eq!(seen.awaited.len(), 1);
        (progressed, seen.awaited[0])
    }

    #[test]
    fn await_input_shortens_a_long_recv_body_timeout_to_the_poll_interval() {
        let (progressed, passed) =
            awaited_with(timeout(Timeout::RecvBody, Duration::from_secs(86_400)));
        assert!(progressed);
        assert_eq!(passed.reason, Timeout::RecvBody);
        assert_eq!(*passed.after, EVERY);
    }

    #[test]
    fn await_input_keeps_a_recv_body_timeout_at_or_under_the_interval() {
        for after in [Duration::ZERO, Duration::from_millis(50), EVERY] {
            let input = timeout(Timeout::RecvBody, after);
            let (_, passed) = awaited_with(input);
            assert_eq!(passed, input);
        }
    }

    #[test]
    fn await_input_leaves_every_other_reason_untouched() {
        for reason in [
            Timeout::Global,
            Timeout::PerCall,
            Timeout::Resolve,
            Timeout::Connect,
            Timeout::SendRequest,
            Timeout::Await100,
            Timeout::SendBody,
            Timeout::RecvResponse,
        ] {
            let input = timeout(reason, Duration::from_secs(3_600));
            let (progressed, passed) = awaited_with(input);
            assert!(!progressed);
            assert_eq!(passed, input);
        }
    }

    #[test]
    fn transmit_output_passes_amount_and_timeout_through() {
        let (mut transport, seen) = polled(true, false);
        let input = timeout(Timeout::SendRequest, Duration::from_secs(3_600));
        transport.transmit_output(17, input).unwrap();
        assert!(matches!(
            transport.transmit_output(0, input),
            Err(Error::ConnectionFailed)
        ));
        assert_eq!(
            seen.lock().unwrap().transmitted,
            vec![(17, input), (0, input)]
        );
        assert!(seen.lock().unwrap().awaited.is_empty());
    }

    #[test]
    fn is_open_and_is_tls_delegate() {
        for (open, tls) in [(true, false), (false, true), (true, true), (false, false)] {
            let (mut transport, _) = polled(open, tls);
            assert_eq!(transport.is_open(), open);
            assert_eq!(transport.is_tls(), tls);
        }
    }

    #[test]
    fn buffers_are_the_inner_transports() {
        let (mut transport, _) = polled(true, false);
        transport.buffers().input_append_buf()[..3].copy_from_slice(b"abc");
        transport.buffers().input_appended(3);
        assert_eq!(transport.inner.buffers.input(), b"abc");
        transport.buffers().input_consume(1);
        assert_eq!(transport.inner.buffers().input(), b"bc");
    }
}
