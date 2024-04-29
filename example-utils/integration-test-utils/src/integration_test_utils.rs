use crate::UPClientFoo;
use async_broadcast::{Receiver, Sender};
use async_std::sync::{Condvar, Mutex};
use async_std::task;
use log::{debug, error};
use rand::random;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};
use up_rust::{UListener, UMessage, UStatus, UTransport, UUIDBuilder, UUri};

const CLEAR_RAND_B: u64 = 0x6000_0000_0000_0000;

pub type Signal = Arc<(Mutex<bool>, Condvar)>;

pub type ActiveConnections = Vec<bool>;
#[derive(PartialEq)]
pub enum ClientCommand {
    NoOp,
    Stop,
    DisconnectedFromStreamer(ActiveConnections),
    ConnectedToStreamer(ActiveConnections),
}

pub async fn check_send_receive_message_discrepancy(
    number_of_sent_messages: u64,
    number_of_messages_received: u64,
    percentage_slack: f64,
) {
    assert!(number_of_sent_messages > 0);
    assert!(number_of_messages_received > 0);

    let slack_in_message_count = number_of_sent_messages as f64 * percentage_slack;
    println!("slack_in_message_count: {slack_in_message_count}");
    println!("number_of_sent_messages: {number_of_sent_messages} number_of_messages_received: {number_of_messages_received}");
    if f64::abs(number_of_sent_messages as f64 - number_of_messages_received as f64)
        > slack_in_message_count
    {
        panic!("The discrepancy between number_of_sent_messages and number_of_message_received \
                is higher than allowable slack: number_of_sent_messages: {number_of_sent_messages}, \
                number_of_messages_received: {number_of_messages_received}, slack_in_message_count: \
                {slack_in_message_count}");
    }
}

pub async fn check_messages_in_order(messages: Arc<Mutex<Vec<UMessage>>>) {
    let messages = messages.lock().await;
    if messages.is_empty() {
        return;
    }

    // Step 1: Group messages by id.lsb
    let mut grouped_messages: HashMap<u64, Vec<&UMessage>> = HashMap::new();
    for msg in messages.iter() {
        let lsb = msg.attributes.as_ref().unwrap().id.lsb;
        grouped_messages.entry(lsb).or_default().push(msg);
    }

    // Step 2: Check each group for strict increasing order of id.msb
    for (lsb, group) in grouped_messages {
        debug!("lsb: {lsb}");
        if let Some(first_msg) = group.first() {
            let mut prev_msb = first_msg.attributes.as_ref().unwrap().id.msb;
            for msg in group.iter().skip(1) {
                let curr_msb = msg.attributes.as_ref().unwrap().id.msb;
                debug!("prev_msb: {prev_msb}, curr_msb: {curr_msb}");
                if curr_msb <= prev_msb {
                    panic!("!! -- Message ordering issue for lsb: {} -- !!", lsb);
                }
                prev_msb = curr_msb;
            }
        }
    }
}

#[inline(always)]
fn override_lsb_rand_b(lsb: u64, new_rand_b: u64) -> u64 {
    lsb & CLEAR_RAND_B | new_rand_b
}

pub async fn wait_for_pause(signal: Signal) {
    let (lock, cvar) = &*signal;
    let mut has_paused = lock.lock().await;
    while !*has_paused {
        debug!("inside wait_for_pause entered while loop");
        // Wait until the client signals it has paused
        has_paused = cvar.wait(has_paused).await;
        debug!("received has_paused notification");
    }
    debug!("exiting wait_for_pause");
}

pub async fn signal_to_pause(signal: Signal) {
    let (lock, cvar) = &*signal;
    let mut should_pause = lock.lock().await;
    *should_pause = true; // Indicate the client should pause
    cvar.notify_all(); // Wake up the client so it can check the condition and continue
}

pub async fn signal_to_resume(signal: Signal) {
    let (lock, cvar) = &*signal;
    let mut should_pause = lock.lock().await;
    *should_pause = false; // Indicate the client should no longer pause
    cvar.notify_all(); // Wake up the client so it can check the condition and continue
}

pub async fn reset_pause(signal: Signal) {
    {
        let (lock, _cvar) = &*signal;
        let mut has_paused = lock.lock().await;
        *has_paused = false;
    }
}

#[allow(clippy::too_many_arguments)]
pub async fn run_client(
    name: String,
    my_client_uuri: UUri,
    listener: Arc<dyn UListener>,
    tx: Sender<Result<UMessage, UStatus>>,
    rx: Receiver<Result<UMessage, UStatus>>,
    mut notification_msgs: Vec<UMessage>,
    mut request_msgs: Vec<UMessage>,
    mut response_msgs: Vec<UMessage>,
    send: bool,
    pause_execution: Signal,
    execution_paused: Signal,
    client_command: Arc<Mutex<ClientCommand>>,
    number_of_sends: Arc<AtomicU64>,
    sent_message_vec_capacity: usize,
) -> JoinHandle<Vec<UMessage>> {
    std::thread::spawn(move || {
        task::block_on(async move {
            let client = UPClientFoo::new(&name, rx, tx).await;

            let client_rand_b = random::<u64>() >> 2;

            let mut sent_messages = Vec::with_capacity(sent_message_vec_capacity);

            let register_res = client
                .register_listener(my_client_uuri.clone(), listener)
                .await;
            let Ok(_registration_string) = register_res else {
                panic!("Unable to register!");
            };

            let mut active_connection_listing = Vec::new();

            let start = Instant::now();

            loop {
                debug!("top of loop");

                {
                    // allows us to pause execution upon command and then signal back when we've done so
                    let (lock, cvar) = &*pause_execution;
                    let mut should_pause = lock.lock().await;
                    while *should_pause {
                        task::sleep(Duration::from_millis(100)).await;

                        let command = client_command.lock().await;
                        if *command == ClientCommand::Stop {
                            let times: u64 = client.times_received.load(Ordering::SeqCst);
                            println!("{name} had rx of: {times}");
                            task::sleep(Duration::from_millis(1000)).await;
                            return sent_messages;
                        } else {
                            match &*command {
                                ClientCommand::NoOp => {}
                                ClientCommand::ConnectedToStreamer(active_connections) => {
                                    debug!("{} commmand: ConnectedToStreamer", &name);
                                    active_connection_listing = active_connections.clone();
                                    debug!(
                                        "{} set connected_to_streamer to: {:?}",
                                        &name, active_connection_listing
                                    );
                                }
                                ClientCommand::DisconnectedFromStreamer(active_connections) => {
                                    debug!("{} commmand: DisconnectedFromStreamer", &name);
                                    active_connection_listing = active_connections.clone();
                                    debug!(
                                        "{} set connected_to_streamer to: {:?}",
                                        &name, active_connection_listing
                                    );
                                }
                                _ => {
                                    error!(
                                        "{} ClientCommand::Stop should have been handled earlier",
                                        &name
                                    )
                                }
                            }
                            {
                                let (lock_exec_pause, cvar_exec_pause) = &*execution_paused;
                                let mut has_paused = lock_exec_pause.lock().await;
                                *has_paused = true;
                                debug!("{} has_paused set to true", &name);
                                cvar_exec_pause.notify_one();
                                debug!("{} cvar_exec_pause.notify_one()", &name);
                            }
                            debug!("{} Loop paused. Waiting...", &name);
                            should_pause = cvar.wait(should_pause).await;
                            debug!("{} Got signal to pause", &name);
                        }
                    }
                }

                let current = Instant::now();
                let ellapsed = current - start;

                debug!("ellapsed: {}", ellapsed.as_millis());

                debug!("-----------------------------------------------------------------------");

                if !send {
                    continue;
                }

                for (index, notification_msg) in &mut notification_msgs.iter_mut().enumerate() {
                    if let Some(attributes) = notification_msg.attributes.as_mut() {
                        let new_id = UUIDBuilder::build();
                        attributes.id.0 = Some(Box::new(new_id));
                        let uuid = attributes.id.as_mut().unwrap();
                        let lsb = &mut uuid.lsb;
                        *lsb = override_lsb_rand_b(*lsb, client_rand_b);
                    }

                    debug!(
                        "prior to sending from client {}, the request message: {:?}",
                        &name, &notification_msg
                    );

                    let send_res = client.send(notification_msg.clone()).await;
                    if send_res.is_err() {
                        error!("Unable to send from client: {}", &name);
                    } else if !active_connection_listing.is_empty()
                        && active_connection_listing[index]
                    {
                        sent_messages.push(notification_msg.clone());
                        number_of_sends.fetch_add(1, Ordering::SeqCst);
                        debug!(
                            "{} after Notification send, we have sent: {}",
                            &name,
                            number_of_sends.load(Ordering::SeqCst)
                        );
                    }
                }

                for (index, request_msg) in &mut request_msgs.iter_mut().enumerate() {
                    if let Some(attributes) = request_msg.attributes.as_mut() {
                        let new_id = UUIDBuilder::build();
                        attributes.id.0 = Some(Box::new(new_id));
                        let uuid = attributes.id.as_mut().unwrap();
                        let lsb = &mut uuid.lsb;
                        *lsb = override_lsb_rand_b(*lsb, client_rand_b);
                    }

                    debug!(
                        "prior to sending from client {}, the request message: {:?}",
                        &name, &request_msg
                    );

                    sent_messages.push(request_msg.clone());

                    let send_res = client.send(request_msg.clone()).await;
                    if send_res.is_err() {
                        error!("Unable to send from client: {}", &name);
                    } else if !active_connection_listing.is_empty()
                        && active_connection_listing[index]
                    {
                        sent_messages.push(request_msg.clone());
                        number_of_sends.fetch_add(1, Ordering::SeqCst);
                        debug!(
                            "{} after Request send, we have sent: {}",
                            &name,
                            number_of_sends.load(Ordering::SeqCst)
                        );
                    }
                }

                for (index, response_msg) in &mut response_msgs.iter_mut().enumerate() {
                    if let Some(attributes) = response_msg.attributes.as_mut() {
                        let new_id = UUIDBuilder::build();
                        attributes.id.0 = Some(Box::new(new_id));
                        let uuid = attributes.id.as_mut().unwrap();
                        let lsb = &mut uuid.lsb;
                        *lsb = override_lsb_rand_b(*lsb, client_rand_b);
                    }

                    debug!(
                        "prior to sending from client {}, the response message: {:?}",
                        &name, &response_msg
                    );

                    sent_messages.push(response_msg.clone());

                    let send_res = client.send(response_msg.clone()).await;
                    if send_res.is_err() {
                        error!("Unable to send from client: {}", &name);
                    } else if !active_connection_listing.is_empty()
                        && active_connection_listing[index]
                    {
                        sent_messages.push(response_msg.clone());
                        number_of_sends.fetch_add(1, Ordering::SeqCst);
                        debug!(
                            "{} after Response send, we have sent: {}",
                            &name,
                            number_of_sends.load(Ordering::SeqCst)
                        );
                    }
                }

                task::sleep(Duration::from_millis(1)).await;
            }
        })
    })
}
