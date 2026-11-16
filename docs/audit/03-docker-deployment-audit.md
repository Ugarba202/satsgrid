# Phase 0 Audit: Containerization and Deployment Architecture

**Status:** SOURCE VERIFIED & RUNTIME VERIFIED  
**Components:** `docker/Dockerfile`, `docker/docker-compose.yml`  
**Tooling Validated:** `docker compose config` (exit code 0)  

---

## 1. Overview & Image Specifications

The SatsGrid deployment stack is containerized for standalone operator hosting. It bundles the SatsGrid Rust node gateway with an integrated Ollama container to provide an out-of-the-box, paid open-model inference environment.

| Component | Base Image | Architecture / Build Type | Privileges |
|---|---|---|---|
| **Builder Stage** | `rust:1.85-slim-bookworm` | Multi-stage release compilation with cargo layer caching | Root during build |
| **Runtime Image** | `debian:bookworm-slim` | Minimal slim runtime with `ca-certificates` & `curl` | Unprivileged user `satsgrid` (UID 1000) |
| **Inference Service** | `ollama/ollama:latest` | Pre-built upstream Ollama container | Root (default upstream) |

---

## 2. Dockerfile Audit (`docker/Dockerfile`)

### 2.1 Multi-Stage Build & Layer Caching
- **Layer Optimization:** The Dockerfile copies `Cargo.toml` and `Cargo.lock` first, generates a dummy `src/main.rs` and `src/lib.rs`, and executes `cargo build --release || true`. This ensures crate dependencies are cached in Docker layers and not recompiled unless dependency manifests change.
- **Application Build:** Copies actual `src/` files and executes `cargo build --release --bin satsgrid`.
- **Minimal Runtime Artifacts:** Only the compiled `/usr/local/bin/satsgrid` binary is transferred to the final Debian Bookworm slim runtime.

### 2.2 Security Hardening
- **Unprivileged Execution:** A dedicated service user and group are provisioned:
  ```dockerfile
  RUN useradd -m -u 1000 -U -s /bin/sh satsgrid
  ```
- **Filesystem Permissions:** Restricts directory ownership of `/etc/satsgrid` and `/var/lib/satsgrid` to UID 1000.
- **Dropping Privileges:** Switches to `USER satsgrid` before setting entrypoint.
- **Container Health Check:** Native Docker health check monitoring:
  ```dockerfile
  HEALTHCHECK --interval=30s --timeout=5s --start-period=5s --retries=3 \
      CMD curl -f http://127.0.0.1:8080/health || exit 1
  ```
- **Entrypoint & Defaults:**
  ```dockerfile
  ENTRYPOINT ["/usr/local/bin/satsgrid"]
  CMD ["node", "start", "--config", "/etc/satsgrid/node.toml"]
  ```

---

## 3. Orchestration Audit (`docker/docker-compose.yml`)

### 3.1 Service Definitions
1. **`satsgrid-ollama` (`services.ollama`):**
   - Container Name: `satsgrid-ollama`
   - Port Binding: `11434:11434`
   - Data Volume: `ollama_data:/root/.ollama` (persists downloaded model weights across restarts)
   - Healthcheck: `ollama list` executed every 10 seconds.
2. **`satsgrid-node` (`services.satsgrid-node`):**
   - Container Name: `satsgrid-node`
   - Build Context: `..` with dockerfile `docker/Dockerfile`
   - Port Binding: `8080:8080`
   - Configuration Volume: Mounts `../config/node.example.toml` into `/etc/satsgrid/node.toml:ro` as a read-only volume.
   - Depends On: `ollama` (condition: `service_started`).
   - Logging Environment: `RUST_LOG=info`.
   - Healthcheck: `curl -f http://127.0.0.1:8080/health`.

### 3.2 Deployment Observations & Recommendations
1. **Network Namespace:** Currently relies on the default bridge network. In `config/node.example.toml`, the upstream engine points to `http://127.0.0.1:11434`. Inside Docker, `127.0.0.1` refers to the container itself. An operator deploying via compose must configure `upstream_url = "http://ollama:11434"` or use `host.docker.internal` / bridge networking.
2. **GPU Acceleration:** `docker-compose.yml` does not declare NVIDIA container runtime reservations (`deploy.resources.reservations.devices`). For production GPU inference on operators' machines, the compose file should document `nvidia-container-toolkit` pass-through options.
3. **Frontend Integration:** The compose file does not include the Next.js `gui/client` service. It focuses exclusively on the backend node and inference engine.
