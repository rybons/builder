// Copyright (c) 2017 Chef Software Inc. and/or applicable contributors
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

#![cfg_attr(feature = "clippy", feature(plugin))]
#![cfg_attr(feature = "clippy", plugin(clippy))]

#[macro_use]
extern crate bitflags;
#[macro_use]
extern crate features;
#[macro_use]
extern crate log;
#[macro_use]
extern crate serde_derive;

use builder_core as bldr_core;
use habitat_builder_db as db;
use habitat_builder_protocol as protocol;
use habitat_core as hab_core;

pub mod config;
pub mod data_store;
pub mod error;

use std::{collections::HashMap,
          fs::File,
          io::Write,
          iter::FromIterator};

use clap::{App,
           Arg};
use copperline::Copperline;
use time::PreciseTime;

use crate::{bldr_core::package_graph::PackageGraph,
            config::Config,
            data_store::DataStore,
            hab_core::config::ConfigFile};

const VERSION: &str = include_str!(concat!(env!("OUT_DIR"), "/VERSION"));

fn main() {
    env_logger::init();

    let matches =
        App::new("bldr-graph").version(VERSION)
                              .about("Habitat Graph Dev Tool")
                              .arg(Arg::with_name("config").help("Filepath to configuration file")
                                                           .required(false)
                                                           .index(1))
                              .get_matches();

    let config = match matches.value_of("config") {
        Some(cfg_path) => Config::from_file(cfg_path).unwrap(),
        None => Config::default(),
    };

    enable_features(&config);

    let mut cl = Copperline::new();

    println!("Connecting to {}", config.datastore.database);

    let datastore = DataStore::new(&config);
    datastore.setup().unwrap();

    println!("Building graph... please wait.");

    let mut graph = PackageGraph::new();
    let packages = datastore.get_job_graph_packages().unwrap();
    let start_time = PreciseTime::now();
    let (ncount, ecount) = graph.build(packages.into_iter(), feat::is_enabled(feat::BuildDeps));
    let end_time = PreciseTime::now();

    println!("OK: {} nodes, {} edges ({} sec)",
             ncount,
             ecount,
             start_time.to(end_time));

    println!("\nAvailable commands: help, stats, top, find, resolve, filter, rdeps, deps, check, \
              exit\n",);

    let mut filter = String::from("");
    let mut done = false;

    while !done {
        let line = cl.read_line_utf8("command> ").ok();
        if line.is_none() {
            continue;
        }
        let cmd = line.expect("Could not get line");
        cl.add_history(cmd.clone());

        let v: Vec<&str> = cmd.trim_end().split_whitespace().collect();

        if !v.is_empty() {
            match v[0].to_lowercase().as_str() {
                "help" => do_help(),
                "stats" => do_stats(&graph),
                "top" => {
                    let count = if v.len() < 2 {
                        10
                    } else {
                        v[1].parse::<usize>().unwrap()
                    };
                    do_top(&graph, count);
                }
                "filter" => {
                    if v.len() < 2 {
                        filter = String::from("");
                        println!("Removed filter\n");
                    } else {
                        filter = String::from(v[1]);
                        println!("New filter: {}\n", filter);
                    }
                }
                "find" => {
                    if v.len() < 2 {
                        println!("Missing search term\n")
                    } else {
                        let max = if v.len() > 2 {
                            v[2].parse::<usize>().unwrap()
                        } else {
                            10
                        };
                        do_find(&graph, v[1].to_lowercase().as_str(), max)
                    }
                }
                "resolve" => {
                    if v.len() < 2 {
                        println!("Missing package name\n")
                    } else {
                        do_resolve(&graph, v[1].to_lowercase().as_str())
                    }
                }
                "rdeps" => {
                    if v.len() < 2 {
                        println!("Missing package name\n")
                    } else {
                        let max = if v.len() > 2 {
                            v[2].parse::<usize>().unwrap()
                        } else {
                            10
                        };
                        do_rdeps(&graph, v[1].to_lowercase().as_str(), &filter, max)
                    }
                }
                "deps" => {
                    if v.len() < 2 {
                        println!("Missing package name\n")
                    } else {
                        do_deps(&datastore, &graph, v[1].to_lowercase().as_str(), &filter)
                    }
                }
                "check" => {
                    if v.len() < 2 {
                        println!("Missing package name\n")
                    } else {
                        do_check(&datastore, &graph, v[1].to_lowercase().as_str(), &filter)
                    }
                }
                "export" => {
                    if v.len() < 2 {
                        println!("Missing file name\n")
                    } else {
                        do_export(&graph, v[1].to_lowercase().as_str(), &filter)
                    }
                }
                "exit" => done = true,
                _ => println!("Unknown command\n"),
            }
        }
    }
}

fn do_help() {
    println!("Commands:");
    println!("  help                    Print this message");
    println!("  stats                   Print graph statistics");
    println!("  top     [<count>]       Print nodes with the most reverse dependencies");
    println!("  filter  [<origin>]      Filter outputs to the specified origin");
    println!("  resolve <name>          Find the most recent version of the package 'origin/name'");
    println!("  find    <term> [<max>]  Find packages that match the search term, up to max items");
    println!("  rdeps   <name> [<max>]  Print the reverse dependencies for the package, up to max");
    println!("  deps    <name>|<ident>  Print the forward dependencies for the package");
    println!("  check   <name>|<ident>  Validate the latest dependencies for the package");
    println!("  export  <filename>      Export data from graph to specified file");
    println!("  exit                    Exit the application\n");
}

fn do_stats(graph: &PackageGraph) {
    let stats = graph.stats();

    println!("Node count: {}", stats.node_count);
    println!("Edge count: {}", stats.edge_count);
    println!("Connected components: {}", stats.connected_comp);
    println!("Is cyclic: {}", stats.is_cyclic);
}

fn do_top(graph: &PackageGraph, count: usize) {
    let start_time = PreciseTime::now();
    let top = graph.top(count);
    let end_time = PreciseTime::now();

    println!("OK: {} items ({} sec)\n",
             top.len(),
             start_time.to(end_time));

    for (name, count) in top {
        println!("{}: {}", name, count);
    }
    println!();
}

fn do_find(graph: &PackageGraph, phrase: &str, max: usize) {
    let start_time = PreciseTime::now();
    let mut v = graph.search(phrase);
    let end_time = PreciseTime::now();

    println!("OK: {} items ({} sec)\n", v.len(), start_time.to(end_time));

    if v.is_empty() {
        println!("No matching packages found")
    } else {
        if v.len() > max {
            v.drain(max..);
        }
        for s in v {
            println!("{}", s);
        }
    }

    println!();
}

fn do_resolve(graph: &PackageGraph, name: &str) {
    let start_time = PreciseTime::now();
    let result = graph.resolve(name);
    let end_time = PreciseTime::now();

    println!("OK: ({} sec)\n", start_time.to(end_time));

    match result {
        Some(s) => println!("{}", s),
        None => println!("No matching packages found"),
    }

    println!();
}

fn do_rdeps(graph: &PackageGraph, name: &str, filter: &str, max: usize) {
    let start_time = PreciseTime::now();

    match graph.rdeps(name) {
        Some(rdeps) => {
            let end_time = PreciseTime::now();
            let mut filtered: Vec<(String, String)> =
                rdeps.into_iter()
                     .filter(|&(ref x, _)| x.starts_with(filter))
                     .collect();

            println!("OK: {} items ({} sec)\n",
                     filtered.len(),
                     start_time.to(end_time));

            if filtered.len() > max {
                filtered.drain(max..);
            }

            if !filter.is_empty() {
                println!("Results filtered by: {}", filter);
            }

            for (s1, s2) in filtered {
                println!("{} ({})", s1, s2);
            }
        }
        None => println!("No entries found"),
    }

    println!();
}

fn resolve_name(graph: &PackageGraph, name: &str) -> String {
    let parts: Vec<&str> = name.split('/').collect();
    if parts.len() == 2 {
        match graph.resolve(name) {
            Some(s) => s,
            None => String::from(name),
        }
    } else {
        String::from(name)
    }
}

fn do_deps(datastore: &DataStore, graph: &PackageGraph, name: &str, filter: &str) {
    let start_time = PreciseTime::now();
    let ident = resolve_name(graph, name);

    println!("Dependencies for: {}", ident);

    match datastore.get_job_graph_package(&ident) {
        Ok(package) => {
            let end_time = PreciseTime::now();
            println!("OK: {} items ({} sec)\n",
                     package.get_deps().len(),
                     start_time.to(end_time));

            if !filter.is_empty() {
                println!("Results filtered by: {}\n", filter);
            }

            for dep in package.get_deps() {
                if dep.to_string().starts_with(filter) {
                    println!("{}", dep)
                }
            }
        }
        Err(_) => println!("No matching package found"),
    }

    println!();
}

fn short_name(ident: &str) -> String {
    let parts: Vec<&str> = ident.split('/').collect();
    assert!(parts.len() >= 2);
    format!("{}/{}", parts[0], parts[1])
}

fn do_check(datastore: &DataStore, graph: &PackageGraph, name: &str, filter: &str) {
    let start_time = PreciseTime::now();
    let mut deps_map = HashMap::new();
    let mut new_deps = Vec::new();
    let ident = resolve_name(graph, name);

    match datastore.get_job_graph_package(&ident) {
        Ok(package) => {
            if !filter.is_empty() {
                println!("Checks filtered by: {}\n", filter);
            }

            println!("Dependecy version updates:");
            for dep in package.get_deps() {
                if dep.to_string().starts_with(filter) {
                    let dep_name = short_name(&dep.to_string());
                    let dep_latest = resolve_name(graph, &dep_name);
                    deps_map.insert(dep_name, dep_latest.clone());
                    new_deps.push(dep_latest.clone());
                    println!("{} -> {}", dep, dep_latest);
                }
            }

            println!();

            for new_dep in new_deps {
                check_package(datastore, &mut deps_map, &new_dep, filter);
            }
        }
        Err(_) => println!("No matching package found"),
    }

    let end_time = PreciseTime::now();
    println!("\nTime: {} sec\n", start_time.to(end_time));
}

fn check_package(datastore: &DataStore,
                 deps_map: &mut HashMap<String, String>,
                 ident: &str,
                 filter: &str) {
    match datastore.get_job_graph_package(ident) {
        Ok(package) => {
            for dep in package.get_deps() {
                if dep.to_string().starts_with(filter) {
                    let name = short_name(&dep.to_string());
                    {
                        let entry = deps_map.entry(name).or_insert_with(|| dep.to_string());
                        if *entry != dep.to_string() {
                            println!("Conflict: {}", ident);
                            println!("  {}", *entry);
                            println!("  {}", dep);
                        }
                    }
                    check_package(datastore, deps_map, &dep.to_string(), filter);
                }
            }
        }
        Err(_) => println!("No matching package found for {}", ident),
    };
}

fn do_export(graph: &PackageGraph, filename: &str, filter: &str) {
    let start_time = PreciseTime::now();
    let latest = graph.latest();
    let end_time = PreciseTime::now();
    println!("\nTime: {} sec\n", start_time.to(end_time));

    let mut file = File::create(filename).expect("Failed to initialize file");

    if !filter.is_empty() {
        println!("Checks filtered by: {}\n", filter);
    }

    for ident in latest {
        if ident.starts_with(filter) {
            file.write_fmt(format_args!("{}\n", ident)).unwrap();
        }
    }
}

fn enable_features(config: &Config) {
    let features: HashMap<_, _> = HashMap::from_iter(vec![("BUILDDEPS", feat::BuildDeps)]);
    let features_enabled = config.features_enabled
                                 .split(',')
                                 .map(|f| f.trim().to_uppercase());

    for key in features_enabled {
        if features.contains_key(key.as_str()) {
            info!("Enabling feature: {}", key);
            feat::enable(features[key.as_str()]);
        }
    }

    if feat::is_enabled(feat::List) {
        println!("Listing possible feature flags: {:?}", features.keys());
        println!("Enable features by populating 'features_enabled' in config");
    }
}

features! {
    pub mod feat {
        const List = 0b0000_0001,
        const BuildDeps = 0b0000_0010
    }
}
