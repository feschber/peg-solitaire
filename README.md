# An Interactive Solver for Peg Solitaire (Englisch Board)


This project implements a fast multithreaded solver for peg-solitaire,
capable of calculating all solvable constellations in ~1s.



## Interactive Solver

Based on the calculated solution graph, an interactive solver is included, showing which moves
lead to a solvable or unsolvable constellation.

<img width="2560" height="1440" alt="image" src="https://github.com/user-attachments/assets/ac79f5d4-64fc-49a2-801b-09da90b932a9" />


This solver is available [online](https://peg-solitaire.feschber.de) as a WASM base WebApp
and can be built for Desktop or Android (as of right now).


## Building for Android
To build the Android App:

Install `cargo-ndk`:
```sh
cargo install --locked cargo-ndk
```

Install necessary toolchains:
```sh
rustup target add aarch64-linux-android
# optional (for x64 / 32-bit arm):
rustup target add x86_64-linux-android armv7-linux-androideabi
```

Compile `peg_solitaire.so` native shared-library:
```sh
cargo ndk -t arm64-v8a -o app/src/main/jniLibs build --package solitaire-game --release
# optional (for x64 / 32-bit arm):
cargo ndk -t arm64-v8a -t x86_64 -t armeabi-v7a -o app/src/main/jniLibs build --package solitaire-game --release
```

Build the app:
```sh
./gradlew assemble
# apk will be at app/build/outputs/apk/release/app-release.apk
```

