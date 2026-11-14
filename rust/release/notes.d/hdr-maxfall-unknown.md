# HDR light levels
- **HDR streams from a monitor that reports no full-frame brightness keep their brightness hints.** Rubylight 4.3.3 sent such streams a MaxFALL of 1 nit, and the Dolby Vision and HDR10+ metadata sent before the first measured frame took the picture as nearly black. MaxFALL stays "unknown" again, so they use the 203-nit reference white; MaxCLL still follows the monitor's peak.
