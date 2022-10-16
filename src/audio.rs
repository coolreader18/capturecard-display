use std::cell::{Cell, RefCell};
use std::future::{self, Future};
use std::io::{Read, Write};
use std::os::unix::io::AsRawFd;
use std::rc::Rc;
use std::sync::Arc;
use std::task::{Context, Poll, Wake, Waker};

use futures_util::{FutureExt, Stream, StreamExt};
use pulse::context::{self, introspect};
use pulse::error::PAErr;
use pulse::mainloop::standard::Mainloop;
use pulse::mainloop::{self, api::Mainloop as _};

pub fn audio_loop(done_ch: (flume::Sender<()>, flume::Receiver<()>)) {
    let rt = PaRuntime::new();
    let mut ctx = rt.make_context("meeee");
    rt.run(async move {
        connect(&mut ctx, None, pulse::context::FlagSet::NOFLAGS, None).await?;
        let mut events = subscribe(&mut ctx, context::subscribe::InterestMaskSet::SOURCE);
        let mut module_id = None;
        let module_loop = async {
            loop {
                let source_index = loop {
                    match find_and_load_module(&mut ctx.introspect()).await {
                        Ok(Some((mod_id, idx))) => {
                            module_id = Some(mod_id);
                            break Some(idx);
                        }
                        Ok(None) => break None,
                        Err(e) => {
                            eprintln!("error setting up audio {e}");
                        }
                    }
                };
                loop {
                    let (_facility, op, index) = events.next().await.unwrap();
                    match op {
                        context::subscribe::Operation::New if source_index.is_none() => break,
                        context::subscribe::Operation::Removed if Some(index) == source_index => {
                            module_id = None;
                            break;
                        }
                        _ => {}
                    }
                }
                // let mod_id =
            }
        };
        futures_util::select_biased! {
            _ = done_ch.1.recv_async() => {},
            x = module_loop.fuse() => {
                enum Never {}
                let x: Never = x;
                match x {}
            }
        }
        if let Some(mod_id) = module_id {
            if !unload_module(&mut ctx.introspect(), mod_id).await {
                eprintln!("failed unloading module {mod_id}")
            }
        }
        let _ = done_ch.0.try_send(());
        Ok::<_, PAErr>(())
    })
    .unwrap()
}

async fn find_and_load_module(
    introspect: &mut introspect::Introspector,
) -> anyhow::Result<Option<(u32, u32)>> {
    let source_info = get_source_info_list(introspect, |info| {
        let form_factor = info.proplist.get("device.form_factor").map(|b| {
            if let Some((b'\0', s)) = b.split_last() {
                s
            } else {
                b
            }
        });
        // TODO: better way of linking video input with audio input
        (form_factor == Some(b"webcam"))
            .then(|| (info.name.as_deref().map(|x| x.to_owned()), info.index))
    })
    .await;
    let (_name, index) = match source_info.into_iter().next() {
        Some(x) => x,
        None => {
            eprintln!("couldn't find suitable audio device");
            return Ok(None);
        }
    };

    let mod_id = load_module(
        introspect,
        "module-loopback",
        &format!("source={index} source_dont_move=true"),
    )
    .await;

    Ok(Some((mod_id, index)))
}

async fn connect(
    ctx: &mut context::Context,
    server: Option<&str>,
    flags: context::FlagSet,
    api: Option<&pulse::def::SpawnApi>,
) -> Result<(), PAErr> {
    ctx.connect(server, flags, api)?;
    future::poll_fn(|cx| match ctx.get_state() {
        context::State::Ready => {
            ctx.set_state_callback(None);
            Poll::Ready(Ok(()))
        }
        context::State::Failed | context::State::Terminated => {
            ctx.set_state_callback(None);
            Poll::Ready(Err(ctx.errno()))
        }
        _ => {
            let waker = cx.waker().clone();
            ctx.set_state_callback(Some(Box::new(move || waker.wake_by_ref())));
            Poll::Pending
        }
    })
    .await
}

async fn get_source_info_list<
    F: FnMut(&introspect::SourceInfo) -> Option<U> + 'static,
    U: 'static,
>(
    introspect: &introspect::Introspector,
    mut f: F,
) -> Vec<U> {
    let v = Rc::new(RefCell::new(Vec::new()));
    let v2 = v.clone();
    let mut op = introspect.get_source_info_list(move |l| {
        if let pulse::callbacks::ListResult::Item(x) = l {
            if let Some(x) = f(x) {
                v2.borrow_mut().push(x)
            }
        }
    });
    let mut v = Some(v);
    wake_on_op(&mut op, move |_| match Rc::try_unwrap(v.take().unwrap()) {
        Ok(x) => Poll::Ready(x.into_inner()),
        Err(x) => {
            v = Some(x);
            Poll::Pending
        }
    })
    .await
}

fn subscribe(
    ctx: &mut context::Context,
    mask: context::subscribe::InterestMaskSet,
) -> impl Stream<
    Item = (
        context::subscribe::Facility,
        context::subscribe::Operation,
        u32,
    ),
> {
    ctx.subscribe(mask, |_| {});
    let (tx, rx) = flume::bounded(32);
    ctx.set_subscribe_callback(Some(Box::new(move |facility, op, idx| {
        // TODO: cancel somehow?
        let _ = tx.try_send((facility.unwrap(), op.unwrap(), idx));
    })));
    rx.into_stream()
}

struct ReturnSlot<T> {
    slot: Rc<Cell<Option<T>>>,
}
impl<T> ReturnSlot<T> {
    fn new() -> Self {
        Self {
            slot: Rc::new(Cell::new(None)),
        }
    }
    fn callback(&self) -> impl FnMut(T) {
        let mut slot = Some(self.slot.clone());
        move |val| match slot.take() {
            Some(slot) => slot.set(Some(val)),
            None => eprintln!("return cb called multiple times"),
        }
    }
    async fn wait(self, mut op: pulse::operation::Operation<impl ?Sized>) -> T {
        wake_on_op(&mut op, |_| match self.slot.take() {
            Some(val) => Poll::Ready(val),
            None => Poll::Pending,
        })
        .await
    }
}

async fn load_module(introspect: &mut introspect::Introspector, name: &str, argument: &str) -> u32 {
    let ret = ReturnSlot::new();
    let op = introspect.load_module(name, argument, ret.callback());
    ret.wait(op).await
}
async fn unload_module(introspect: &mut introspect::Introspector, id: u32) -> bool {
    let ret = ReturnSlot::new();
    let op = introspect.unload_module(id, ret.callback());
    ret.wait(op).await
}

async fn wake_on_op<T>(
    op: &mut pulse::operation::Operation<impl ?Sized>,
    mut f: impl FnMut(&mut Context<'_>) -> Poll<T>,
) -> T {
    future::poll_fn(|cx| {
        let poll = f(cx);
        if poll.is_ready() {
            op.set_state_callback(None);
        } else {
            if op.get_state() == pulse::operation::State::Cancelled {
                panic!("cancelled")
            }
            let waker = cx.waker().clone();
            op.set_state_callback(Some(Box::new(move || waker.wake_by_ref())));
        }
        poll
    })
    .await
}

struct PaWaker {
    pipe: os_pipe::PipeWriter,
}
impl Wake for PaWaker {
    fn wake(self: Arc<Self>) {
        self.wake_by_ref();
    }

    fn wake_by_ref(self: &Arc<Self>) {
        let _ = (&self.pipe).write(b"X");
    }
}

pub struct PaRuntime {
    waker: Arc<PaWaker>,
    wakee: os_pipe::PipeReader,
    looop: Mainloop,
}
impl PaRuntime {
    pub fn new() -> Self {
        let (wakee, pipe) = os_pipe::pipe().unwrap();
        let waker = Arc::new(PaWaker { pipe });
        let looop = Mainloop::new().unwrap();
        Self {
            waker,
            wakee,
            looop,
        }
    }
    pub fn make_context(&self, name: &str) -> context::Context {
        context::Context::new(&self.looop, name).unwrap()
    }
    pub fn run<R: 'static>(mut self, fut: impl Future<Output = R> + 'static) -> R {
        let mut fut = Box::pin(fut);
        let ret = Rc::new(Cell::new(None));
        let mut ret2 = Some(ret.clone());
        let mut looop2 = Mainloop {
            _inner: self.looop._inner.clone(),
        };
        let waker = Waker::from(self.waker);
        let ev = self.looop.new_deferred_event(Box::new(move |mut ev| {
            ev.disable();
            let mut cx = Context::from_waker(&waker);
            let poll = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                fut.as_mut().poll(&mut cx)
            })) {
                Ok(poll) => poll,
                Err(x) => {
                    ret2.take().unwrap().set(Some(Err(x)));
                    looop2.quit(pulse::def::Retval(111));
                    return;
                }
            };
            if let Poll::Ready(ret) = poll {
                ret2.take().unwrap().set(Some(Ok(ret)));
                looop2.quit(pulse::def::Retval(0));
            }
        }));
        let mut ev = ev.unwrap();
        let _ev = self.looop.new_io_event(
            self.wakee.as_raw_fd(),
            mainloop::events::io::FlagSet::INPUT,
            Box::new(move |_, _, _| {
                let mut buf = [0u8; 16];
                while let Ok(16) = self.wakee.read(&mut buf) {}
                ev.enable();
            }),
        );
        // ev
        self.looop.run().unwrap();
        let res = Rc::try_unwrap(ret)
            .unwrap_or_else(|_| panic!("aaaaa"))
            .into_inner()
            .unwrap();
        match res {
            Ok(ret) => ret,
            Err(e) => std::panic::resume_unwind(e),
        }
    }
}
