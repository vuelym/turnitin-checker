use std::env;
use std::path::Path;

fn main() {
    let target = env::var("TARGET").unwrap();
    
    if target.contains("windows") {
        // Only use winres when targeting Windows
        let mut res = winres::WindowsResource::new();
        
        // Set the icon
        res.set_icon("resources/app_icon.ico");
        
        // Set application metadata
        res.set("FileDescription", "Turnitin Checker");
        res.set("ProductName", "Turnitin Checker");
        res.set("ProductVersion", "1.1.0");
        res.set("FileVersion", "1.1.0");
        res.set("LegalCopyright", "Copyright © 2023");
        
        // Compile the resource
        res.compile().unwrap();
    }
    
    // Ensure the resources directory is included in the package
    println!("cargo:rerun-if-changed=resources/app_icon.ico");
}
