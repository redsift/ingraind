#![allow(non_camel_case_types)]

use std::collections::HashMap;
use std::ptr;
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use std::fs::metadata;
use std::os::unix::fs::MetadataExt;

use redbpf::{Map, Module, VoidPtr};

use grains::*;

include!(concat!(env!("OUT_DIR"), "/file.rs"));

type ino_t = u64;

const ACTION_IGNORE: u8 = 0;
const ACTION_RECORD: u8 = 1;

fn find_map_by_name<'a>(module: &'a mut Module, needle: &str) -> (usize, &'a mut Map) {
    module
        .maps
        .iter_mut()
        .enumerate()
        .find(|(i, v)| v.name == needle)
        .unwrap()
}

pub struct Files {
    files: Arc<Mutex<HashMap<ino_t, FileAccess>>>,
    actionlist: HashMap<String, String>,
    backends: Vec<BackendHandler>,
    volumes: Option<Map>,
}

#[derive(Debug)]
pub struct FileAccess {
    pub id: u64,
    pub process: String,
    pub path: String,
    pub ino: ino_t,
}

impl Files {
    pub fn new() -> Files {
        Files {
            files: Arc::new(Mutex::new(HashMap::new())),
            actionlist: HashMap::new(),
            backends: vec![],
            volumes: None,
        }
    }
}

impl EBPFGrain<'static> for Files {
    fn loaded(&mut self, module: &mut Module) {
        let (volidx, _) = find_map_by_name(module, "volumes");

        {
            let (_, actionlist) = find_map_by_name(module, "actionlist");

            let mut record = _data_action {
                action: ACTION_RECORD,
            };
            let mut ino = metadata("/").unwrap().ino();
            actionlist.set(
                &mut ino as *mut ino_t as VoidPtr,
                &mut record as *mut _data_action as VoidPtr,
            );
        }

        let volumes = module.maps.swap_remove(volidx);
        let files = self.files.clone();
        thread::spawn(move || loop {
            thread::sleep(Duration::from_secs(5));

            let mut hash = files.lock().unwrap();
            for mut k in volumes.iter::<ino_t>() {
                let mut ino = Rc::make_mut(&mut k);

                match hash.get_mut(ino) {
                    Some(x) => println!("{:?}", x),
                    None => println!("File not found"),
                }

                let ptr = ino as *mut ino_t as VoidPtr;

                volumes.delete(ptr);
            }
        });
    }

    fn attached(&mut self, backends: &[BackendHandler]) {
        self.backends.extend_from_slice(backends);
    }

    fn code() -> &'static [u8] {
        include_bytes!(concat!(env!("OUT_DIR"), "/file.elf"))
    }

    fn get_handler(&self, id: &str) -> EventCallback {
        let files = self.files.clone();
        Box::new(move |raw| {
            let file = FileAccess::from(_data_file::from(raw));
            let mut hash = files.lock().unwrap();
            hash.entry(file.ino).or_insert(file);

            // Not a goal to generate an event at this stage.
            // Events will be generated by the background thread for this grain.
            None
        })
    }
}

impl From<_data_file> for FileAccess {
    fn from(data: _data_file) -> FileAccess {
        let ino = data.path[0].ino;
        let mut path_segments = data.path.to_vec();
        path_segments.reverse();
        let path = path_segments
            .iter()
            .map(|s| to_string(unsafe { &*(&s.name as *const [i8] as *const [u8]) }))
            .collect::<Vec<String>>()
            .join("/");

        FileAccess {
            id: data.id,
            process: to_string(unsafe { &*(&data.comm as *const [i8] as *const [u8]) }),
            path,
            ino,
        }
    }
}