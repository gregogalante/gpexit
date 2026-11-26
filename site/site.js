// Resolves direct asset URLs from the latest GitHub release. Links fall back to
// the releases page when the API is unavailable or rate limited.
const API = 'https://api.github.com/repos/gregogalante/gpexit/releases/latest'

const LABELS = {
  'macos-arm64.dmg': 'Download for macOS',
  'windows-x64-setup.exe': 'Download for Windows',
  'linux-x64.AppImage': 'Download for Linux'
}

function detectAsset () {
  const platform = (navigator.userAgentData?.platform || navigator.platform || navigator.userAgent).toLowerCase()
  if (/android|iphone|ipad/.test(platform + navigator.userAgent.toLowerCase())) return null
  // Browsers do not expose Apple Silicon vs Intel reliably; Apple Silicon is the default.
  if (platform.includes('mac')) return 'macos-arm64.dmg'
  if (platform.includes('win')) return 'windows-x64-setup.exe'
  if (platform.includes('linux')) return 'linux-x64.AppImage'
  return null
}

async function loadRelease () {
  const response = await fetch(API, { headers: { Accept: 'application/vnd.github+json' } })
  if (!response.ok) throw new Error(`GitHub API ${response.status}`)
  return response.json()
}

function applyRelease (release) {
  const urlFor = suffix => release.assets.find(asset => asset.name.endsWith(suffix))?.browser_download_url

  for (const link of document.querySelectorAll('[data-asset]')) {
    const url = urlFor(link.dataset.asset)
    if (url) link.href = url
  }

  const version = release.tag_name
  const releaseLink = document.getElementById('release-link')
  releaseLink.textContent = `${release.name || version}`
  releaseLink.href = release.html_url

  const suffix = detectAsset()
  const url = suffix && urlFor(suffix)
  if (url) {
    const primary = document.getElementById('primary-download')
    primary.href = url
    primary.textContent = LABELS[suffix]
    document.getElementById('release-note').textContent = `${version} · Other platforms below`
  }
}

loadRelease().then(applyRelease).catch(error => console.warn('Release lookup failed:', error))
