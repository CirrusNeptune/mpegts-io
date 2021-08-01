use mpegts_io::{BdavParser, Payload};
use pretty_env_logger;
use std::env;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};

fn main() {
    pretty_env_logger::init();
    let args = env::args();
    if args.len() < 2 {
        panic!("No file argument");
    }
    let file_path = args.skip(1).next().unwrap();

    let mut file = File::open(file_path).expect("unable to open!");
    let len = file.seek(SeekFrom::End(0)).expect("unable to get file size");
    file.seek(SeekFrom::Start(0)).expect("unable to get file size");
    let num_packets = len / 192;
    let mut parser = BdavParser::default();
    for _ in 0..num_packets {
        let mut packet = [0_u8; 192];
        file.read_exact(&mut packet).expect("IO Error!");
        let parsed_packet = parser.parse(&packet).expect("Parse Error!");
        match parsed_packet.packet.adaptation_field {
            Some(_) => {
                println!("{:x?}", parsed_packet);
            }
            None => {}
        }
        match parsed_packet.packet.payload {
            Some(ref payload) => match payload {
                Payload::PesPending => {}
                _ => {
                    println!("{:x?}", parsed_packet);
                }
            },
            None => {}
        }
    }
}
