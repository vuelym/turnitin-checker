# Turnitin Credential Checker

[![Rust](https://img.shields.io/badge/Rust-1.75+-blue.svg)](https://www.rust-lang.org/)
[![License](https://img.shields.io/badge/License-MIT-green.svg)](LICENSE)
[![Build Status](https://img.shields.io/github/workflow/status/yourusername/turnitin-checker/Rust)](https://github.com/yourusername/turnitin-checker/actions)
[![Latest Release](https://img.shields.io/github/v/release/yourusername/turnitin-checker)](https://github.com/yourusername/turnitin-checker/releases)

A high-performance Rust application for checking Turnitin credentials in parallel. This application features:

- Multi-threading for maximum efficiency
- User-friendly GUI with file selection
- Detailed results with status indicators
- CSV export functionality

## 🚀 Features

- **High Performance**: Uses Rust's concurrency features and asynchronous programming
- **Easy to Use**: Simple GUI interface for selecting credential files and output location
- **Detailed Results**: Shows whether credentials are valid and retrieves user information
- **Multi-threaded**: Automatically uses all available CPU cores for maximum throughput
- **Cross-Platform**: Works on Windows, macOS, and Linux

## 📋 Prerequisites

- Rust 1.75 or higher
- Cargo (Rust's package manager)
- Git (for cloning the repository)

## ��️ Installation

### Precompiled Binaries (Recommended)

Precompiled binaries for Windows, macOS, and Linux are available in the [Releases](https://github.com/yourusername/turnitin-checker/releases) section. Simply download the appropriate version for your operating system and run the executable.

### From Source

1. Clone the repository:
   ```bash
   git clone https://github.com/yourusername/turnitin-checker.git
   cd turnitin-checker
   ```

2. Build the application:
   ```bash
   cargo build --release
   ```

3. Run the application:
   ```bash
   cargo run --release
   ```

## 💻 Usage

1. Launch the application
2. Click "Browse" next to "Credentials File" to select a text file with credentials
   - File format should be `username:password` with one credential per line
3. Click "Browse" next to "Output File" to specify where to save the CSV results
4. Adjust the number of threads (default is optimal for your system)
5. Click "Start" to begin the checking process
6. Results will be displayed and saved to the specified CSV file

### Example Credential File

```plaintext
user1@example.com:password123
user2@example.com:password456
user3@example.com:password789
```

## 📊 Performance

This application is designed to be extremely efficient, using:

- Non-blocking I/O operations
- Thread pooling
- Parallel credential processing
- Connection pooling and reuse
- Optimized HTTP requests with gzip compression
- Efficient memory usage

## 📦 Dependencies

- tokio - Asynchronous runtime
- reqwest - HTTP client
- eframe/egui - GUI framework
- rayon - Parallel processing
- And more (see Cargo.toml for full list)

## 🤝 Contributing

Contributions are welcome! Please feel free to submit a Pull Request.

1. Fork the repository
2. Create your feature branch (`git checkout -b feature/amazing-feature`)
3. Commit your changes (`git commit -m 'Add some amazing feature'`)
4. Push to the branch (`git push origin feature/amazing-feature`)
5. Open a Pull Request

## 📝 License

This project is licensed under the MIT License - see the [LICENSE](LICENSE) file for details.

## ⚠️ Disclaimer

This tool is for educational purposes only. Use responsibly and in accordance with Turnitin's terms of service. The developers are not responsible for any misuse of this software.

## 📞 Support

If you encounter any issues or have questions, please open an issue in the GitHub repository. 
