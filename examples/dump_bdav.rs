use mpegts_io::{bdav::BdavParser, Payload};
use pretty_env_logger;
use std::env;
use std::fs::File;
use std::io::{Read, Result, Seek, SeekFrom};

fn file_size(file: &mut File) -> Result<u64> {
    let len = file.seek(SeekFrom::End(0))?;
    file.seek(SeekFrom::Start(0))?;
    Ok(len)
}

fn main() {
    pretty_env_logger::init();
    let args = env::args();
    if args.len() < 2 {
        panic!("No file argument");
    }
    let file_path = args.skip(1).next().unwrap();
    let mut file = File::open(file_path).expect("unable to open!");
    let num_packets = file_size(&mut file).expect("unable to get file size") / 192;
    let mut parser = BdavParser::default();
    for _ in 0..num_packets {
        let mut packet = [0_u8; 192];
        file.read_exact(&mut packet).expect("IO Error!");
        let parsed_packet = parser.parse(&packet).expect("Parse Error!");
        match parsed_packet.packet.adaptation_field {
            Some(_) => {
                println!("{:#x?}", parsed_packet);
                continue;
            }
            None => {}
        }
        match parsed_packet.packet.payload {
            Some(ref payload) => match payload {
                Payload::PesPending => {}
                _ => {
                    println!("{:#x?}", parsed_packet);
                    continue;
                }
            },
            None => {}
        }
    }
}
