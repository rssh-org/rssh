use super::*;
use std::sync::Mutex as TestMutex;
use tokio::sync::Notify;

struct MockPort {
    events: Arc<TestMutex<Vec<String>>>,
    entered: Arc<Notify>,
    release_write: Option<std::sync::mpsc::Receiver<()>>,
    release_break: Option<std::sync::mpsc::Receiver<()>>,
}

impl Read for MockPort {
    fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
        unreachable!("the writer adapter must not read")
    }
}

impl Write for MockPort {
    fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
        self.events
            .lock()
            .unwrap()
            .push(String::from_utf8_lossy(data).into());
        self.entered.notify_one();
        if let Some(release) = self.release_write.take() {
            release
                .recv_timeout(Duration::from_secs(2))
                .expect("async worker must remain able to release the write");
        }
        Ok(data.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl SerialPort for MockPort {
    fn name(&self) -> Option<String> {
        Some("mock".into())
    }
    fn baud_rate(&self) -> serialport::Result<u32> {
        Ok(9600)
    }
    fn data_bits(&self) -> serialport::Result<DataBits> {
        Ok(DataBits::Eight)
    }
    fn flow_control(&self) -> serialport::Result<FlowControl> {
        Ok(FlowControl::None)
    }
    fn parity(&self) -> serialport::Result<Parity> {
        Ok(Parity::None)
    }
    fn stop_bits(&self) -> serialport::Result<StopBits> {
        Ok(StopBits::One)
    }
    fn timeout(&self) -> Duration {
        Duration::from_millis(100)
    }
    fn set_baud_rate(&mut self, _: u32) -> serialport::Result<()> {
        Ok(())
    }
    fn set_data_bits(&mut self, _: DataBits) -> serialport::Result<()> {
        Ok(())
    }
    fn set_flow_control(&mut self, _: FlowControl) -> serialport::Result<()> {
        Ok(())
    }
    fn set_parity(&mut self, _: Parity) -> serialport::Result<()> {
        Ok(())
    }
    fn set_stop_bits(&mut self, _: StopBits) -> serialport::Result<()> {
        Ok(())
    }
    fn set_timeout(&mut self, _: Duration) -> serialport::Result<()> {
        Ok(())
    }
    fn write_request_to_send(&mut self, level: bool) -> serialport::Result<()> {
        self.events.lock().unwrap().push(format!("rts:{level}"));
        Ok(())
    }
    fn write_data_terminal_ready(&mut self, level: bool) -> serialport::Result<()> {
        self.events.lock().unwrap().push(format!("dtr:{level}"));
        Ok(())
    }
    fn read_clear_to_send(&mut self) -> serialport::Result<bool> {
        Ok(false)
    }
    fn read_data_set_ready(&mut self) -> serialport::Result<bool> {
        Ok(false)
    }
    fn read_ring_indicator(&mut self) -> serialport::Result<bool> {
        Ok(false)
    }
    fn read_carrier_detect(&mut self) -> serialport::Result<bool> {
        Ok(false)
    }
    fn bytes_to_read(&self) -> serialport::Result<u32> {
        Ok(0)
    }
    fn bytes_to_write(&self) -> serialport::Result<u32> {
        Ok(0)
    }
    fn clear(&self, _: serialport::ClearBuffer) -> serialport::Result<()> {
        Ok(())
    }
    fn try_clone(&self) -> serialport::Result<Box<dyn SerialPort>> {
        unreachable!("the writer adapter must not clone the native port")
    }
    fn set_break(&self) -> serialport::Result<()> {
        self.events.lock().unwrap().push("break".into());
        self.entered.notify_one();
        if let Some(release) = &self.release_break {
            release
                .recv_timeout(Duration::from_secs(2))
                .expect("async worker must remain able to release BREAK");
        }
        Ok(())
    }
    fn clear_break(&self) -> serialport::Result<()> {
        self.events.lock().unwrap().push("clear".into());
        Ok(())
    }
}

fn mock_handle(
    release_write: Option<std::sync::mpsc::Receiver<()>>,
    release_break: Option<std::sync::mpsc::Receiver<()>>,
) -> (SerialHandle, Arc<TestMutex<Vec<String>>>, Arc<Notify>) {
    let events = Arc::new(TestMutex::new(Vec::new()));
    let entered = Arc::new(Notify::new());
    let handle = SerialHandle {
        writer: Arc::new(Mutex::new(Box::new(MockPort {
            events: events.clone(),
            entered: entered.clone(),
            release_write,
            release_break,
        }))),
        _guard: Arc::new(CloseGuard {
            closed: Arc::new(AtomicBool::new(false)),
        }),
        port_name: Arc::from("mock"),
    };
    (handle, events, entered)
}

#[tokio::test(flavor = "current_thread")]
async fn break_pulse_leaves_async_worker_available() {
    let (release, receiver) = std::sync::mpsc::channel();
    let (handle, events, entered) = mock_handle(None, Some(receiver));
    let pulse = handle.send_break();
    let observer = async {
        entered.notified().await;
        assert_eq!(
            *events.lock().unwrap(),
            ["break"],
            "another async task must run before BREAK is released"
        );
        release.send(()).unwrap();
    };
    let (result, ()) = tokio::join!(pulse, observer);
    result.unwrap();
    assert_eq!(*events.lock().unwrap(), ["break", "clear"]);
}

#[tokio::test(flavor = "current_thread")]
async fn blocked_write_keeps_async_worker_free_and_cancelled_call_keeps_its_lock() {
    let (release, receiver) = std::sync::mpsc::channel();
    let (handle, events, entered) = mock_handle(Some(receiver), None);
    let first_handle = handle.clone();
    let first = tokio::spawn(async move { first_handle.write(b"first").await });
    entered.notified().await;
    first.abort();
    assert!(first.await.unwrap_err().is_cancelled());

    let second = handle.write(b"second");
    let dtr = handle.set_dtr(true);
    let rts = handle.set_rts(false);
    tokio::pin!(second, dtr, rts);
    assert!(futures_util::poll!(&mut second).is_pending());
    assert!(futures_util::poll!(&mut dtr).is_pending());
    assert!(futures_util::poll!(&mut rts).is_pending());
    assert_eq!(*events.lock().unwrap(), ["first"]);
    release.send(()).unwrap();

    let (second, dtr, rts) = tokio::join!(second, dtr, rts);
    second.unwrap();
    dtr.unwrap();
    rts.unwrap();
    assert_eq!(
        *events.lock().unwrap(),
        ["first", "second", "dtr:true", "rts:false"]
    );
}
