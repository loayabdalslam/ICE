# Add a one-command ICE install to any website

These entry points resolve `LATEST` and fetch the matching versioned binary.

| Platform | Script |
| --- | --- |
| Windows | `https://raw.githubusercontent.com/loayabdalslam/ice-binaries/main/install.ps1` |
| Linux | `https://raw.githubusercontent.com/loayabdalslam/ice-binaries/main/bash/install.sh` |
| macOS | `https://raw.githubusercontent.com/loayabdalslam/ice-binaries/main/mac/install.sh` |
| Linux / macOS auto-detection | `https://raw.githubusercontent.com/loayabdalslam/ice-binaries/main/install.sh` |

## Commands

Windows PowerShell:

```powershell
irm https://raw.githubusercontent.com/loayabdalslam/ice-binaries/main/install.ps1 | iex
```

Linux:

```bash
curl -fsSL https://raw.githubusercontent.com/loayabdalslam/ice-binaries/main/bash/install.sh | bash
```

macOS:

```bash
curl -fsSL https://raw.githubusercontent.com/loayabdalslam/ice-binaries/main/mac/install.sh | bash
```

## Copy buttons — plain HTML

Works on an HTTPS website, without a framework. Visitors choose their operating system.
The page copies a command; the visitor runs it in their terminal.

```html
<section aria-label="Install ICE">
  <h2>Your next idea starts here.</h2>
  <p>Install ICE in one command.</p>
  <button data-ice="windows">Copy Windows command</button>
  <button data-ice="linux">Copy Linux command</button>
  <button data-ice="macos">Copy macOS command</button>
  <pre><code id="ice-command">Choose your operating system.</code></pre>
  <p id="ice-copy-status" role="status" aria-live="polite"></p>
</section>
<script>
  const root = 'https://raw.githubusercontent.com/loayabdalslam/ice-binaries/main';
  const commands = {
    windows: `irm ${root}/install.ps1 | iex`,
    linux: `curl -fsSL ${root}/bash/install.sh | bash`,
    macos: `curl -fsSL ${root}/mac/install.sh | bash`
  };
  document.querySelectorAll('[data-ice]').forEach(button => {
    button.addEventListener('click', async () => {
      const command = commands[button.dataset.ice];
      document.getElementById('ice-command').textContent = command;
      try {
        await navigator.clipboard.writeText(command);
        document.getElementById('ice-copy-status').textContent = 'Copied. Paste into your terminal to install ICE.';
      } catch {
        document.getElementById('ice-copy-status').textContent = 'Select and copy the command above.';
      }
    });
  });
</script>
```

When embedding in a site with a strict Content Security Policy, put the script in
your site's own JS file. The clipboard API requires HTTPS or localhost.

## Pin a version for reproducible setup

```bash
curl -fsSL https://raw.githubusercontent.com/loayabdalslam/ice-binaries/main/install.sh | bash -s -- --version 0.2.0
```

For Windows, set `$env:ICE_VERSION = '0.2.0'` before running the one-liner.
For a mirror, set `ICE_BASE_URL` to its HTTPS root; keep the same `LATEST` and
`releases/VERSION/` directory structure.
