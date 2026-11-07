#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    if std::env::args().any(|a| a == "--phone-standby") {
        ptcc_lib::standby::main()
    } else {
        ptcc_lib::run()
    }
}
