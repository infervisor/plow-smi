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

  config = mkIf cfg.enable {
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
      };
    };
  };
}
