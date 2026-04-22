#!/bin/bash
patch -p1 << 'PATCH'
--- a/src/main.rs
+++ b/src/main.rs
@@ -10,6 +10,7 @@
 use std::io;
 use std::os::unix::process::CommandExt;
 use std::path::Path;
+use std::os::unix::fs::MetadataExt;
 use std::process::Command;
 use std::time::{Duration, Instant};
 use wait_timeout::ChildExt;
@@ -159,16 +160,33 @@

 fn lock_file(lock_filename: &Path, lock_timeout: Duration) -> Result<File, String> {
     let start = Instant::now();
-    let file = OpenOptions::new()
-        .write(true)
-        .create(true)
-        .truncate(false)
-        .open(lock_filename)
-        .map_err(|e| e.to_string())?;
-
+    let mut file = OpenOptions::new()
+            .write(true)
+            .create(true)
+            .truncate(false)
+            .open(lock_filename)
+            .map_err(|e| e.to_string())?;
     loop {
         match file.try_lock_exclusive() {
-            Ok(true) => return Ok(file),
+            Ok(true) => {
+                let file_meta = file.metadata().map_err(|e| e.to_string())?;
+                let path_meta_result = std::fs::metadata(lock_filename);
+
+                match path_meta_result {
+                    Ok(path_meta) if file_meta.dev() == path_meta.dev() && file_meta.ino() == path_meta.ino() => {
+                        return Ok(file);
+                    }
+                    _ => {
+                        // The file we locked is not the one currently at lock_filename.
+                        // Re-open and loop again to avoid acquiring a lock on a deleted/replaced file.
+                        file = OpenOptions::new()
+                            .write(true)
+                            .create(true)
+                            .truncate(false)
+                            .open(lock_filename)
+                            .map_err(|e| e.to_string())?;
+                    }
+                }
+            }
             Ok(false) => {
                 if start.elapsed() >= lock_timeout {
                     return Err(
PATCH
