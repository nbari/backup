# backup

Command line tool for creating encrypted backups avoiding duplicates.

[![crates.io](https://img.shields.io/crates/v/backup.svg)](https://crates.io/crates/backup)
[![Build Status](https://travis-ci.org/nbari/backup.svg?branch=master)](https://travis-ci.org/nbari/backup)

## Usage

Create a new backup of /home/user1 and /home/user2

```bash
backup new mybackup -d /home/user1 -d /home/user2
```
