#!/usr/bin/env python3
"""Operate isolated staging/production projects on one Docker host; never source env files."""
import argparse
import os
from pathlib import Path
import re
import subprocess
import sys
from urllib.parse import urlsplit, unquote, parse_qs

HERE = Path(__file__).resolve().parent
ENVIRONMENTS = ("staging", "production")
# Worker-only cloud-computer provider keys; mirrors ProviderId::api_key_env.
PROVIDER_KEYS = ("BOXD_API_KEY", "BOAT_API_KEY", "E2B_API_KEY", "DAYTONA_API_KEY", "PRIZED_API_KEY")


def domain(value):
    if len(value) > 253 or not re.fullmatch(
        r"(?:[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?\.)+[a-z]{2,63}", value
    ):
        raise ValueError("Use a lowercase DNS hostname, without scheme, port or path")
    return value


def read_env(path):
    result = {}
    for line in path.read_text().splitlines():
        if not line.strip() or line.lstrip().startswith("#"):
            continue
        key, separator, value = line.partition("=")
        if not separator or not re.fullmatch(r"[A-Z][A-Z0-9_]*", key) or key in result:
            raise ValueError(f"Invalid or duplicate setting in {path.name}")
        result[key] = value
    return result


def private_write(path, content):
    with open(path, "x", opener=lambda p, f: os.open(p, f, 0o600)) as target:
        target.write(content)


def initialize(root, production, staging, image):
    domain(production)
    domain(staging)
    if production == staging:
        raise ValueError("Production and staging require different domains")
    if not image or any(c.isspace() for c in image) or any(c in image for c in "'$\"#\\"):
        raise ValueError("Use an immutable image tag or digest without env-file syntax")
    paths = [root / "edge.env"] + [root / e / f for e in ENVIRONMENTS
                                      for f in ("deploy.env", "runtime.env", "worker.env")]
    if any(p.exists() for p in paths):
        raise ValueError("Configuration already exists; edit it explicitly instead of overwriting secrets")
    root.mkdir(parents=True, exist_ok=True, mode=0o700)
    root.chmod(0o700)
    private_write(root / "edge.env", f"PRODUCTION_DOMAIN={production}\nSTAGING_DOMAIN={staging}\n")
    for name, hostname in (("production", production), ("staging", staging)):
        directory = root / name
        directory.mkdir(mode=0o700)
        memory = "384m" if name == "production" else "256m"
        worker_memory = "192m" if name == "production" else "128m"
        port = 18080 if name == "production" else 18081
        private_write(directory / "deploy.env", (
            f"ZUSE_ENV={name}\nZUSE_CLOUD_IMAGE={image}\n"
            f"APP_CPUS=1\nAPP_MEMORY={memory}\nAPP_HOST_PORT={port}\n"
            f"WORKER_CPUS=0.5\nWORKER_MEMORY={worker_memory}\n"
            "AUTH_WORKER_CPUS=0.25\nAUTH_WORKER_MEMORY=256m\n"
            "ZUSE_DATABASE_CA_FILE=/etc/ssl/certs/ca-certificates.crt\n"
        ))
        private_write(directory / "runtime.env", (
            "# Raw Docker env file: literal values, no shell or Compose expansion.\n"
            "# Provide this environment's external Postgres connection URL.\n"
            "ZUSE_CLOUD_DATABASE_URL=\n"
            "# Public HTTPS API origin; API and gateway share this issuer.\n"
            "API_ISSUER=REPLACE_WITH_CONTROL_PLANE_ISSUER\n"
            "WORKOS_CLIENT_ID=REPLACE_WITH_ENVIRONMENT_CLIENT_ID\n"
            "# Set the browser application's exact HTTPS origin(s), comma separated.\n"
            "ZUSE_CLOUD_ALLOWED_ORIGINS=\n"
            "RUST_LOG=info\n"
            "# Runtime enrollment and gateway leases share this environment database.\n"
            "ZUSE_CLOUD_RUNTIME_WRITES=true\n"
            "ZUSE_CLOUD_GATEWAY_MAX_SOCKETS=256\n"
            "ZUSE_CLOUD_GATEWAY_QUEUED_BYTES=33554432\n"
            "# Configure keys only after schema and runtime/API protocol compatibility.\n"
            "# CLOUD_DATA_ENCRYPTION_KEY=\n# RELAY_MINT_PRIVATE_JWK=\n"
            "# Creation is disabled until an explicit test-account allowlist is configured.\n"
            "ZUSE_CLOUD_CREATE_ACCOUNTS=\n"
            "# Enable only with the computers migration and certified authority image.\n"
            "ZUSE_CLOUD_AUTH_WRITE_ACCOUNTS=\n"
        ))
        private_write(directory / "worker.env", "# Provider keys are never supplied to API/auth containers.\n"
                      + "".join(f"{key}=\n" for key in PROVIDER_KEYS) + "ZUSE_CLOUD_WORKER_POLL_MS=1000\n")


def database_identity(raw):
    url = urlsplit(raw)
    if url.scheme not in ("postgres", "postgresql") or not url.hostname or not url.path.strip("/"):
        raise ValueError("Configure a valid external Postgres URL")
    return (url.hostname.lower(), url.port or 5432, unquote(url.path))


def isolated_schema_url(raw, environment):
    url = urlsplit(raw)
    values = parse_qs(url.query)
    return (unquote(url.username or "") == f"zuse_cloud_{environment}"
            and values.get("options") == [f"-csearch_path=zuse_{environment},pg_catalog"]
            and values.get("sslmode") == ["verify-full"])



def api_secret_is_readable(metadata):
    mode = metadata.st_mode & 0o777
    return ((metadata.st_uid == 10001 and mode == 0o400)
            or (metadata.st_uid == 0 and metadata.st_gid == 10001 and mode == 0o440))


def github_api(root, name, runtime):
    if runtime.get("ZUSE_CLOUD_GITHUB_WRITE_ACCOUNTS", "").strip():
        raise ValueError("Native GitHub rollout belongs in API-only api.env, not shared runtime.env")
    path = root / name / "api.env"
    settings = read_env(path) if path.exists() else {}
    if not settings.get("ZUSE_CLOUD_GITHUB_WRITE_ACCOUNTS", "").strip():
        return False
    required = ("GITHUB_APP_ID", "GITHUB_APP_SLUG", "GITHUB_APP_CLIENT_ID", "ZUSE_CLOUD_GITHUB_ORIGIN")
    if any(not settings.get(key, "").strip() for key in required):
        raise ValueError("Native GitHub API configuration is incomplete")
    origin = urlsplit(settings["ZUSE_CLOUD_GITHUB_ORIGIN"])
    if origin.scheme != "https" or not origin.hostname or origin.username or origin.password or origin.path not in ("", "/") or origin.query or origin.fragment:
        raise ValueError("Native GitHub API origin must be an HTTPS origin")
    files = {"GITHUB_APP_PRIVATE_KEY_FILE": "app.pem", "GITHUB_APP_CLIENT_SECRET_FILE": "oauth-client-secret", "GITHUB_WEBHOOK_SECRET_FILE": "webhook-secret"}
    for key, filename in files.items():
        if settings.get(key) != f"/run/zuse/github/{filename}":
            raise ValueError("Use the API-only GitHub secret mount paths")
        secret = root / name / "github-api" / filename
        if not secret.is_file() or secret.is_symlink() or not api_secret_is_readable(secret.stat()):
            raise ValueError("GitHub API secret copies must be UID10001 mode0400 or root:GID10001 mode0440 regular files")
    if any(key in settings for key in ("GITHUB_APP_PRIVATE_KEY", "GITHUB_APP_CLIENT_SECRET", "GITHUB_WEBHOOK_SECRET")):
        raise ValueError("Native GitHub secrets must use API-only private files")
    return True

def signal_directory_is_private(metadata):
    return metadata.st_uid == 10001 and metadata.st_mode & 0o777 == 0o700


def account_events(root, name, runtime):
    if runtime.get("ZUSE_CLOUD_ACCOUNT_EVENTS") != "true":
        return False
    directory = root / name / "event-signals"
    if not directory.is_dir() or directory.is_symlink():
        raise ValueError("Account events require a private environment signal directory")
    metadata = directory.stat()
    if not signal_directory_is_private(metadata):
        raise ValueError("Account event signal directory must be UID10001 mode0700")
    return True


def verification_scratch(root, name, settings):
    import importlib.util
    spec = importlib.util.spec_from_file_location("verification_scratch", HERE / "verification-scratch.py")
    scratch = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(scratch)
    return scratch.inspect(root, name, settings)


def compose(root, name):
    # Do not inherit caller variables that can override --env-file or Docker context.
    # DOCKER_HOST/context remain explicit Docker operator choices; Compose settings do not.
    process_env = {k: v for k, v in os.environ.items()
                   if not k.startswith(("COMPOSE_", "ZUSE_"))}
    if name == "edge":
        settings = read_env(root / "edge.env")
        domain(settings["PRODUCTION_DOMAIN"])
        domain(settings["STAGING_DOMAIN"])
        if settings["PRODUCTION_DOMAIN"] == settings["STAGING_DOMAIN"]:
            raise ValueError("Ingress domains must differ")
        filename = "ingress.yaml"
        env_file = root / "edge.env"
    else:
        env_file = root / name / "deploy.env"
        settings = read_env(env_file)
        if settings.get("ZUSE_ENV") != name:
            raise ValueError("Environment identity does not match selected project")
        runtime = read_env(root / name / "runtime.env")
        url = runtime.get("ZUSE_CLOUD_DATABASE_URL", "")
        if url:
            identity = database_identity(url)
            other = "production" if name == "staging" else "staging"
            other_path = root / other / "runtime.env"
            other_url = read_env(other_path).get("ZUSE_CLOUD_DATABASE_URL", "") if other_path.exists() else ""
            if other_url and database_identity(other_url) == identity:
                # A shared Supabase database is safe only with explicitly isolated
                # schema search paths AND separate restricted login roles.
                other = "production" if name == "staging" else "staging"
                if not isolated_schema_url(url, name) or not isolated_schema_url(other_url, other):
                    raise ValueError("Staging and production require separate external databases or isolated schemas and roles")

        process_env["ZUSE_CONFIG_DIR"] = str(root / name)
        filename = "compose.yaml"
    # Explicitly mask shell values for every key read by Compose.
    for key in settings:
        process_env.pop(key, None)
    command = ["docker", "compose", "--env-file", str(env_file), "--project-name",
               f"zuse-{name}", "--file", str(HERE / filename)]
    if name != "edge" and verification_scratch(root, name, settings) is not None:
        command += ["--file", str(HERE / "verification-scratch.yaml")]
    if name != "edge" and github_api(root, name, runtime):
        command += ["--file", str(HERE / "github-api.yaml")]
    if name != "edge" and account_events(root, name, runtime):
        command += ["--file", str(HERE / "account-events.yaml")]
    if name != "edge" and runtime.get("ZUSE_CLOUD_AUTH_WRITE_ACCOUNTS", "").strip():
        command += ["--profile", "authority"]
    return command, process_env


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--config-root", type=Path, default=Path("/etc/zuse/cloud"))
    commands = parser.add_subparsers(dest="command", required=True)
    init = commands.add_parser("init", help="Generate private config; does not create any cloud resources")
    init.add_argument("--production-domain", required=True)
    init.add_argument("--staging-domain", required=True)
    init.add_argument("--image", required=True)
    for action in ("check", "up", "stop", "status", "logs"):
        cmd = commands.add_parser(action)
        cmd.add_argument("environment", choices=(*ENVIRONMENTS, "edge"))
    args = parser.parse_args()
    root = args.config_root.resolve()
    if args.command == "init":
        initialize(root, args.production_domain, args.staging_domain, args.image)
        print(f"Created private environment files under {root}. Configure external DB, auth and keys before starting services.")
        return
    command, process_env = compose(root, args.environment)
    operations = {
        "check": ["config", "--quiet"],
        "up": ["up", "--detach", "--wait", "--wait-timeout", "180"],
        "stop": ["stop"],
        "status": ["ps"],
        "logs": ["logs", "--tail", "100", "--follow"],
    }
    if args.command == "up" and args.environment != "edge":
        runtime = read_env(root / args.environment / "runtime.env")
        if not runtime.get("ZUSE_CLOUD_DATABASE_URL"):
            raise ValueError("Configure the external Postgres URL before starting services")
        if runtime.get("WORKOS_CLIENT_ID", "").startswith("REPLACE_"):
            raise ValueError("Configure environment-specific WorkOS settings before starting services")
        if not runtime.get("CLOUD_DATA_ENCRYPTION_KEY") or not runtime.get("RELAY_MINT_PRIVATE_JWK"):
            raise ValueError("Configure runtime encryption and signing keys before starting services")
        if runtime.get("API_ISSUER", "").startswith("REPLACE_"):
            raise ValueError("Configure the matching control-plane ticket issuer before starting services")
    if args.command == "up" and args.environment != "edge":
        worker = read_env(root / args.environment / "worker.env")
        if not any(worker.get(key) for key in PROVIDER_KEYS):
            raise ValueError("Configure at least one worker provider key before starting services")
        # Preflight must never drain an allocation, deletion or credential operation.
        subprocess.run(command + ["run", "--rm", "--no-deps", "worker", "worker", "--check"], env=process_env, check=True)
        if runtime.get("ZUSE_CLOUD_AUTH_WRITE_ACCOUNTS", "").strip():
            subprocess.run(command + ["run", "--rm", "--no-deps", "auth-worker", "auth-worker", "--check"],
                           env=process_env, check=True)
    subprocess.run(command + operations[args.command], env=process_env, check=True)


if __name__ == "__main__":
    try:
        main()
    except (ValueError, KeyError, OSError, subprocess.CalledProcessError) as error:
        # Never print expanded Compose configuration or connection strings.
        if isinstance(error, subprocess.CalledProcessError):
            sys.exit(error.returncode)
        if isinstance(error, KeyError):
            sys.exit("Required deployment setting is missing")
        sys.exit(str(error))
