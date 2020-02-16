use futures::executor::block_on;
use std::future::Future;
use std::pin::Pin;
use std::sync::mpsc;
use std::sync::mpsc::{Receiver, SyncSender};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, Waker};
use std::thread;

fn main() {
    Game::new().run();
}

type PathfinderFunction = dyn FnOnce(&Pathfinder) -> usize + Send + Sync;

struct Pathfinder {
    tx: SyncSender<Arc<Mutex<PathfinderFutureState>>>,
    rx: Receiver<Arc<Mutex<PathfinderFutureState>>>,
    paths: usize,
}

impl Pathfinder {
    fn new() -> Pathfinder {
        let (tx, rx) = mpsc::sync_channel(10_000);
        Pathfinder { tx, rx, paths: 0 }
    }

    fn updater(&self) -> PathfinderUpdater {
        PathfinderUpdater {
            tx: self.tx.clone(),
        }
    }

    fn run(&mut self) {
        loop {
            self.do_functions();
        }
    }

    fn do_functions(&mut self) {
        while let Ok(state) = self.rx.try_recv() {
            let mut state = state.lock().unwrap();
            if let Some(function) = state.function.take() {
                thread::sleep_ms(1000);
                self.paths += 1;
                state.output = Some(function(self));
                if let Some(waker) = state.waker.take() {
                    waker.wake()
                }
            }
        }
    }
}

struct PathfinderFutureState {
    output: Option<usize>,
    waker: Option<Waker>,
    function: Option<Box<PathfinderFunction>>,
}

struct PathfinderFuture {
    state: Arc<Mutex<PathfinderFutureState>>,
}

impl Future for PathfinderFuture {
    type Output = usize;
    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let mut shared_state = self.state.lock().unwrap();
        if let Some(output) = shared_state.output {
            Poll::Ready(output)
        } else {
            shared_state.waker = Some(cx.waker().clone());
            Poll::Pending
        }
    }
}

#[derive(Clone)]
struct PathfinderUpdater {
    tx: SyncSender<Arc<Mutex<PathfinderFutureState>>>,
}

impl PathfinderUpdater {
    fn update(&self, function: Box<PathfinderFunction>) -> PathfinderFuture {
        let state = PathfinderFutureState {
            output: None,
            waker: None,
            function: Some(function),
        };
        let state = Arc::new(Mutex::new(state));

        self.tx.send(state.clone()).unwrap();

        PathfinderFuture { state }
    }
}

type GameUpdateFunction = dyn FnOnce(&mut Game) + Send + Sync;

struct GameFuture {
    state: Arc<Mutex<GameFutureState>>,
}

impl Future for GameFuture {
    type Output = ();
    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let mut shared_state = self.state.lock().unwrap();
        if shared_state.completed {
            Poll::Ready(())
        } else {
            shared_state.waker = Some(cx.waker().clone());
            Poll::Pending
        }
    }
}

struct GameFutureState {
    completed: bool,
    waker: Option<Waker>,
    function: Option<Box<GameUpdateFunction>>,
}

#[derive(Clone)]
struct GameUpdater {
    tx: SyncSender<Arc<Mutex<GameFutureState>>>,
}

impl GameUpdater {
    fn update(&self, function: Box<GameUpdateFunction>) -> GameFuture {
        let state = GameFutureState {
            completed: false,
            waker: None,
            function: Some(function),
        };
        let state = Arc::new(Mutex::new(state));

        self.tx.send(state.clone()).unwrap();

        GameFuture { state }
    }
}

struct GameEventConsumer {
    active: Arc<Mutex<bool>>,
    game_updater: GameUpdater,
    pathfinder_updater: PathfinderUpdater,
}

impl GameEventConsumer {
    fn consume(&self, state: &Game) {
        if *self.active.lock().unwrap() {
            print!(".");
            return;
        }
        *self.active.lock().unwrap() = true;
        let game_updater = self.game_updater.clone();
        let pathfinder_updater = self.pathfinder_updater.clone();
        let active = self.active.clone();
        thread::spawn(move || {
            block_on(async {
                let paths = pathfinder_updater
                    .update(Box::new(|pathfinder| {
                        println!("Found path {}", pathfinder.paths);
                        pathfinder.paths
                    }))
                    .await;
                game_updater
                    .update(Box::new(move |game| {
                        game.counter = paths;
                        println!("Set counter to {}", game.counter);
                    }))
                    .await;
                let paths = pathfinder_updater
                    .update(Box::new(|pathfinder| {
                        println!("Found path {}", pathfinder.paths);
                        pathfinder.paths
                    }))
                    .await;
                game_updater
                    .update(Box::new(move |game| {
                        game.counter = paths;
                        println!("Set counter to {}", game.counter);
                    }))
                    .await;
                *active.lock().unwrap() = false;
            })
        });
    }
}

struct Game {
    consumer: GameEventConsumer,
    tx: SyncSender<Arc<Mutex<GameFutureState>>>,
    rx: Receiver<Arc<Mutex<GameFutureState>>>,
    counter: usize,
}

impl Game {
    fn new() -> Game {
        let (tx, rx) = mpsc::sync_channel(10_000);
        let mut pathfinder = Pathfinder::new();
        let pathfinder_updater = pathfinder.updater();
        thread::spawn(move || {
            pathfinder.run();
        });
        Game {
            consumer: GameEventConsumer {
                game_updater: GameUpdater { tx: tx.clone() },
                pathfinder_updater,
                active: Arc::new(Mutex::new(false)),
            },
            tx,
            rx,
            counter: 0,
        }
    }

    fn run(&mut self) {
        loop {
            thread::sleep_ms(10);
            self.events();
            self.mutate();
        }
    }

    fn events(&self) {
        let consumer = &self.consumer;
        consumer.consume(&self);
    }

    fn mutate(&mut self) {
        while let Ok(state) = self.rx.try_recv() {
            let mut state = state.lock().unwrap();
            if let Some(function) = state.function.take() {
                function(self);
                state.completed = true;
                if let Some(waker) = state.waker.take() {
                    waker.wake()
                }
            }
        }
    }
}
