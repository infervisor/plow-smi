self:
{ config, lib, pkgs, ... }:

with lib;

let
  cfg = config.services.plow-smi-exporter;
in
{
  options.services.plow-smi-exporter = {
    enable = mkEnableOption "Plow SMI Prometheus GPU/system metrics exporter";

    package = mkOption {
      type = types.package;
      default = self.packages.${pkgs.system}.plows-exporter;
      defaultText = literalExpression "plow-smi.packages.<system>.plows-exporter";
      description = "The plows-exporter package to run.";
    };

    bind = mkOption {
      type = types.str;
      default = "0.0.0.0";
      description = "Address the exporter's HTTP server listens on.";
    };

    port = mkOption {
      type = types.port;
      default = 9835;
      description = "Port the exporter's HTTP server listens on.";
    };

    nvidia = mkOption {
      type = types.bool;
      default = false;
      description = "Enable NVIDIA GPU metrics collection.";
    };

    amd = mkOption {
      type = types.bool;
      default = false;
      description = "Enable AMD GPU metrics collection.";
    };

    intel = mkOption {
      type = types.bool;
      default = false;
      description = "Enable Intel GPU metrics collection.";
    };

    system = mkOption {
      type = types.bool;
      default = true;
      description = "Enable system (CPU/RAM/disk/network) metrics collection.";
    };

    tpu = mkOption {
      type = types.bool;
      default = false;
      description = "Enable Google Cloud TPU metrics collection.";
    };

    network = mkOption {
      type = types.bool;
      default = true;
      description = "Enable network interface counters (bytes, packets, errors, drops, link state).";
    };

    wireguard = mkOption {
      type = types.bool;
      default = false;
      description = ''
        Enable WireGuard interface and peer metrics. Adds wireguard-tools to the
        unit's PATH and grants CAP_NET_ADMIN, which `wg show` needs.
      '';
    };

    probe = mkOption {
      type = types.listOf types.str;
      default = [ ];
      example = [ "10.100.0.1" ];
      description = "IPv4 addresses to ping every probe interval for loss and round-trip time.";
    };

    probeWireguardPeers = mkOption {
      type = types.bool;
      default = false;
      description = "Ping every WireGuard peer at the first /32 in its allowed IPs. Implies `wireguard`.";
    };

    probeIntervalMs = mkOption {
      type = types.ints.between 200 60000;
      default = 1000;
      description = "Milliseconds between probe rounds.";
    };

    all = mkOption {
      type = types.bool;
      default = false;
      description = "Enable every available collector (overrides the individual vendor/system/tpu flags).";
    };

    interval = mkOption {
      type = types.ints.positive;
      default = 5;
      description = "Metrics collection interval in seconds.";
    };

    logLevel = mkOption {
      type = types.enum [ "trace" "debug" "info" "warn" "error" ];
      default = "info";
      description = "Log level filter.";
    };

    extraFlags = mkOption {
      type = types.listOf types.str;
      default = [ ];
      description = "Extra command-line flags passed to plows-exporter.";
    };

    openFirewall = mkOption {
      type = types.bool;
      default = false;
      description = "Open the configured port in the firewall.";
    };

    user = mkOption {
      type = types.str;
      default = "plow-smi-exporter";
      description = "User account under which the exporter runs.";
    };

    group = mkOption {
      type = types.str;
      default = "plow-smi-exporter";
      description = "Group under which the exporter runs.";
    };
  };

  config = mkIf cfg.enable (let
    wireguard = cfg.all || cfg.wireguard || cfg.probeWireguardPeers;
    probing = cfg.probe != [ ] || cfg.probeWireguardPeers;
  in {
    users.users = mkIf (cfg.user == "plow-smi-exporter") {
      plow-smi-exporter = {
        isSystemUser = true;
        group = cfg.group;
        description = "Plow SMI exporter service user";
      };
    };

    users.groups = mkIf (cfg.group == "plow-smi-exporter") {
      plow-smi-exporter = { };
    };

    networking.firewall.allowedTCPPorts = mkIf cfg.openFirewall [ cfg.port ];

    systemd.services.plow-smi-exporter = {
      description = "Plow SMI Prometheus GPU/system metrics exporter";
      wantedBy = [ "multi-user.target" ];
      after = [ "network-online.target" ];
      wants = [ "network-online.target" ];
      path = optional wireguard pkgs.wireguard-tools;

      serviceConfig = {
        ExecStart = concatStringsSep " " ([
          "${cfg.package}/bin/plows-exporter"
          "--bind" cfg.bind
          "--port" (toString cfg.port)
          "--interval" (toString cfg.interval)
          "--log-level" cfg.logLevel
        ]
        ++ optional cfg.all "--all"
        ++ optional (!cfg.all && cfg.nvidia) "--nvidia"
        ++ optional (!cfg.all && cfg.amd) "--amd"
        ++ optional (!cfg.all && cfg.intel) "--intel"
        ++ optional (!cfg.all && cfg.system) "--system"
        ++ optional (!cfg.all && cfg.tpu) "--tpu"
        ++ optional (!cfg.all && cfg.network) "--network"
        ++ optional (!cfg.all && cfg.wireguard) "--wireguard"
        ++ concatMap (ip: [ "--probe" ip ]) cfg.probe
        ++ optional cfg.probeWireguardPeers "--probe-wireguard-peers"
        ++ optionals probing [ "--probe-interval-ms" (toString cfg.probeIntervalMs) ]
        ++ cfg.extraFlags);

        User = cfg.user;
        Group = cfg.group;
        Restart = "on-failure";
        RestartSec = "5s";

        # Hardening. GPU vendor libraries are dlopen'd from the host at
        # runtime (NVML/Level Zero) and AMD telemetry is read from /sys and
        # /proc (amdgpu sysfs, KFD, DRM fdinfo), so we can't fully sandbox the
        # filesystem — DynamicUser is left off and /dev is left reachable
        # for GPU device nodes.
        NoNewPrivileges = true;
        ProtectSystem = "strict";
        ProtectHome = true;
        PrivateTmp = true;
        SupplementaryGroups = [ "video" "render" ];
        # `wg show` needs CAP_NET_ADMIN; ICMP probes use a ping socket where
        # net.ipv4.ping_group_range allows it, else a raw socket (CAP_NET_RAW).
        AmbientCapabilities =
          optional wireguard "CAP_NET_ADMIN" ++ optional probing "CAP_NET_RAW";
      };
    };
  });
}
