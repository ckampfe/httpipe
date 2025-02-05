#!/usr/bin/env sh

launchctl unload ~/Library/LaunchAgents/ckampfe.httpipe.plist
launchctl load -w ~/Library/LaunchAgents/ckampfe.httpipe.plist
