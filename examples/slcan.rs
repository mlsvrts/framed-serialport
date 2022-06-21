extern crate framed_serial;
extern crate futures;
extern crate tokio;

use framed_serial::{
    slcan::{SlCanCodec, SlCanFrame},
    FramedSerial,
};
use futures::StreamExt;
use tokio::task;

#[tokio::main]
async fn main() {
    // Get the port name
    let mut args = std::env::args();
    let name = args
        .nth(1)
        .expect("the SLCAN serial port to attach must be provided");

    // From a serial port name
    let dev = FramedSerial::try_from_port(name, 115200, SlCanCodec)
        .await
        .expect("failed to create the framed serialport");

    // Handle data frames
    let mut rx = dev.subscribe::<SlCanFrame>();
    task::spawn(async move {
        while let Some(msg) = rx.next().await {
            println!("Data: {:?}", msg);
        }
    });

    // Setup the port
    dev.send(SlCanFrame::close())
        .await
        .expect("failed to close the slcan device");

    dev.send(SlCanFrame::baudrate(1000000).unwrap())
        .await
        .expect("failed to set the slcan device baudrate");

    dev.send(SlCanFrame::open())
        .await
        .expect("failed to open the slcan device");

    // Send some messages
    let res1 = dev
        .send("t123411223344")
        .await
        .expect("failed to send slcan frame");

    let mal = dev
        .send("tBROKENMESSAGE")
        .await
        .expect("failed to send slcan frame");

    let res2 = dev
        .send("t1234AABBCCDD")
        .await
        .expect("failed to send slcan frame");

    // Wait for the results
    let t1 = task::spawn(async move {
        println!("Result  1: {:?}", res1.await);
    });
    let t2 = task::spawn(async move {
        println!("Failure 1: {:?}", mal.await);
    });
    let t3 = task::spawn(async move {
        println!("Result  2: {:?}", res2.await);
    });

    tokio::try_join!(t1, t2, t3).expect("failed to join all tasks");

    // Sleep for a little to print some incoming messages
    tokio::time::sleep(tokio::time::Duration::from_millis(250)).await;
}
