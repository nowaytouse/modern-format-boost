// Direct unit test for Rust Ctrl+C guard
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

// Import our ctrlc_guard module
mod shared_utils {
    pub mod ctrlc_guard {
        include!("shared_utils/src/ctrlc_guard.rs");
    }
}

use shared_utils::ctrlc_guard::{init, should_show_confirmation, set_mock_elapsed_time};

fn main() {
    println!("🧪 Testing Rust Ctrl+C Guard Unit Functions");
    
    // Test 1: Basic initialization
    println!("1. Testing basic initialization...");
    init();
    println!("   ✅ init() completed without error");
    
    // Test 2: Should not show confirmation before threshold
    println!("2. Testing before 4.5 minute threshold...");
    set_mock_elapsed_time(60); // 1 minute
    let result = should_show_confirmation();
    println!("   should_show_confirmation() = {} (should be false)", result);
    assert!(!result, "Should not show confirmation before 4.5 minutes");
    println!("   ✅ Correctly returns false before threshold");
    
    // Test 3: Should show confirmation after threshold
    println!("3. Testing after 4.5 minute threshold...");
    set_mock_elapsed_time(300); // 5 minutes
    let result = should_show_confirmation();
    println!("   should_show_confirmation() = {} (should be true)", result);
    assert!(result, "Should show confirmation after 4.5 minutes");
    println!("   ✅ Correctly returns true after threshold");
    
    // Test 4: Multiple init calls (should be idempotent)
    println!("4. Testing multiple init() calls...");
    init();
    init();
    init();
    println!("   ✅ Multiple init() calls completed without error");
    
    // Test 5: Thread safety
    println!("5. Testing thread safety...");
    let handles: Vec<_> = (0..5)
        .map(|i| {
            thread::spawn(move || {
                set_mock_elapsed_time(300); // 5 minutes
                let result = should_show_confirmation();
                println!("   Thread {}: should_show_confirmation() = {}", i, result);
                result
            })
        })
        .collect();
    
    let mut all_passed = true;
    for (i, handle) in handles.into_iter().enumerate() {
        let result = handle.join().unwrap();
        if !result {
            println!("   ❌ Thread {} returned false!", i);
            all_passed = false;
        }
    }
    
    if all_passed {
        println!("   ✅ All threads returned true (thread safety confirmed)");
    } else {
        println!("   ❌ Thread safety test failed");
        return;
    }
    
    println!("\n🎉 All Rust Ctrl+C Guard unit tests passed!");
    println!("✅ Rust 守卫功能完全正常工作！");
}
