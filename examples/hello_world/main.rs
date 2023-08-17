use futures::channel::mpsc::SendError;
use futures::future::BoxFuture;
use futures::{stream, FutureExt, SinkExt, StreamExt};
use std::collections::HashSet;
use std::sync::Arc;

#[derive(Debug)]
pub enum AktorError {
    SendError,
}

impl From<SendError> for AktorError {
    fn from(_value: SendError) -> Self {
        AktorError::SendError
    }
}

pub type AktorResult<T> = Result<T, AktorError>;

trait MyFirstActor {
    fn add_value(&mut self, value: i32) -> BoxFuture<AktorResult<()>>;

    fn list_values(&mut self) -> BoxFuture<AktorResult<Vec<i32>>>;
}

struct MyFirstActorImpl {
    values: HashSet<i32>,
}

impl MyFirstActor for MyFirstActorImpl {
    fn add_value(&mut self, msg: i32) -> BoxFuture<AktorResult<()>> {
        self.values.insert(msg);

        async move { Ok(()) }.boxed()
    }

    fn list_values(&mut self) -> BoxFuture<AktorResult<Vec<i32>>> {
        let response = self.values.iter().cloned().collect();

        async move { Ok(response) }.boxed()
    }
}

enum MsgSet {
    AV(i32),
    LV,
}

#[derive(Debug)]
enum RSet {
    AV,
    LV(AktorResult<Vec<i32>>),
}

struct Request {
    msg: MsgSet,
    response: Option<futures::channel::oneshot::Sender<RSet>>,
}

struct MyFirstActorLocalMessagingClient {
    send: futures::channel::mpsc::Sender<Request>,
}

impl MyFirstActor for MyFirstActorLocalMessagingClient {
    fn add_value(&mut self, value: i32) -> BoxFuture<AktorResult<()>> {
        async move {
            self.send
                .send(Request {
                    msg: MsgSet::AV(value),
                    response: None,
                })
                .await?;
            Ok(())
        }
        .boxed()
    }

    fn list_values(&mut self) -> BoxFuture<AktorResult<Vec<i32>>> {
        let (send, recv) = futures::channel::oneshot::channel();
        async move {
            self.send
                .send(Request {
                    msg: MsgSet::LV,
                    response: Some(send),
                })
                .await?;

            match recv.await {
                Ok(RSet::LV(v)) => v,
                _ => panic!("Unexpected response"),
            }
        }
        .boxed()
    }
}

struct MyFirstActorLocalMessagingServer {
    recv: futures::channel::mpsc::Receiver<Request>,
    inst: Arc<futures::lock::Mutex<Box<dyn MyFirstActor + Send>>>,
}

impl MyFirstActorLocalMessagingServer {
    fn spawn(self) -> BoxFuture<'static, ()> {
        async move {
            self.recv
                .zip(stream::repeat_with(|| self.inst.clone()))
                .for_each({
                    |(r, inst)| async move {
                        match r.msg {
                            MsgSet::AV(m) => {
                                let mut guard = inst.lock().await;
                                let fut = guard.add_value(m);
                                fut.await.unwrap();
                            }
                            MsgSet::LV => {
                                let resp = inst.lock().await.list_values().await;
                                if let Some(r) = r.response {
                                    r.send(RSet::LV(resp)).unwrap();
                                } else {
                                    panic!("Should be a responding type");
                                }
                            }
                        }
                    }
                })
                .await;
        }
        .boxed()
    }
}

struct MyFirstActorFactory {}

impl MyFirstActorFactory {
    fn new_local_messaging(
        actor: Box<dyn MyFirstActor + Send>,
    ) -> (impl MyFirstActor, MyFirstActorLocalMessagingServer) {
        let (send, recv) = futures::channel::mpsc::channel(10);

        let client = MyFirstActorLocalMessagingClient { send };

        let server = MyFirstActorLocalMessagingServer {
            recv,
            inst: Arc::new(futures::lock::Mutex::new(actor)),
        };

        (client, server)
    }
}

struct MyFirstActorMock {
    handler: fn(MsgSet) -> RSet,
}

impl MyFirstActorMock {
    fn new(handler: fn(MsgSet) -> RSet) -> Self {
        MyFirstActorMock { handler }
    }
}

impl MyFirstActor for MyFirstActorMock {
    fn add_value(&mut self, value: i32) -> BoxFuture<AktorResult<()>> {
        match (self.handler)(MsgSet::AV(value)) {
            RSet::AV => async move { Ok(()) }.boxed(),
            _ => panic!("Unexpected response"),
        }
    }

    fn list_values(&mut self) -> BoxFuture<AktorResult<Vec<i32>>> {
        match (self.handler)(MsgSet::LV) {
            RSet::LV(v) => async move { v }.boxed(),
            _ => panic!("Unexpected response"),
        }
    }
}

#[tokio::main]
async fn main() {
    let actor = MyFirstActorImpl {
        values: HashSet::new(),
    };

    let (mut client, server) = MyFirstActorFactory::new_local_messaging(Box::new(actor));

    tokio::spawn(async move {
        server.spawn().await;
    });

    for i in 0..100 {
        client.add_value(i).await.unwrap();
    }
    println!("{:?}", client.list_values().await.unwrap());

    let mock_actor = MyFirstActorMock::new(|msg| match msg {
        MsgSet::LV => RSet::LV(Ok(vec![1, 2, 3])),
        _ => panic!("Not handled"),
    });

    let (mut client, server) = MyFirstActorFactory::new_local_messaging(Box::new(mock_actor));

    tokio::spawn(async move {
        server.spawn().await;
    });

    println!("{:?}", client.list_values().await.unwrap());
}
