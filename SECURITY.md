# Security

If you find a security problem in SigilSmith, please tell us privately first,
so we can fix it before it is public.

## How to report

- **On GitHub:** the **Report a vulnerability** button on this repository's
  **Security** tab. Only you and we can see the report.
- **By email:** [hello@sigilsmith.com](mailto:hello@sigilsmith.com), with
  "Security" in the subject.

Please don't open a public issue, discussion or Nexus comment about it.

A good report says which version you use (`sigilsmith --version`), how you
installed it (AppImage, `.tar.gz` or built from source), what happens and how
to make it happen. A small example mod or archive helps, if one is involved.

## What happens next

We reply within 14 days to say we have it, and keep you posted until it is
fixed. Once a fixed release is out, we describe the problem in the changelog
and credit you, unless you'd rather not be named. We can't offer a bounty.

## What counts

The latest release of SigilSmith: the desktop app, the TUI and the command
line. For example:

- a mod archive or file that makes SigilSmith write outside the folders listed
  in [What SigilSmith touches](docs/what-sigilsmith-touches.md);
- an update that installs without its checksum matching;
- SigilSmith running a program or command it shouldn't.

Problems in the mods themselves, in the game, in Norbyte's Script Extender or
on Nexus Mods belong with their authors. If you're not sure, send it to us
anyway.

## Testing

Test on your own computer and your own files. Please don't touch other
people's data or systems. If you report in good faith, we won't take action
against you.
