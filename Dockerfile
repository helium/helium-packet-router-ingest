# Use the official Rust image as the base image
FROM rust:latest as builder

# Get protoc
RUN apt-get update && apt-get install -y protobuf-compiler

# Set the working directory
WORKDIR /app

# Copy the Cargo.toml and Cargo.lock files to cache dependencies
COPY Cargo.toml Cargo.lock ./

# Create a dummy project file to build deps around
RUN mkdir src && echo "fn main() {}" > src/main.rs

# Build the dependencies (this will be cached unless the dependencies change)
RUN cargo build --release

# Copy the source code
COPY src ./src
COPY settings ./settings

# Build the application
RUN cargo install --path .

# Use a slim Alpine-based image for the final container
# FROM alpine:latest
FROM debian:bullseye-slim

# Set the working directory
WORKDIR /app

# Copy the built binary from the builder stage
COPY --from=builder /usr/local/cargo/bin/hpr-http-rs /usr/local/bin/hpr-http-rs
COPY settings ./settings

# Expose ports 9000, 9001, and 9002
EXPOSE 9000
EXPOSE 9001
EXPOSE 9002

# Start the Rust program when the container is run
CMD ["hpr-http-rs", "serve", "./settings.toml"]
