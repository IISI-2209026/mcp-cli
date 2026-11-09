#!/bin/bash
# Release script for mcp-cli (Pure Rust Version)
# Usage: ./scripts/release.sh <version>
# Example: ./scripts/release.sh 0.1.0

set -e

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

# Check if version argument is provided
if [ -z "$1" ]; then
    echo -e "${RED}Error: Version number required${NC}"
    echo "Usage: ./scripts/release.sh <version>"
    echo "Example: ./scripts/release.sh 0.1.0"
    exit 1
fi

VERSION=$1

# Validate version format (semver)
if ! [[ $VERSION =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
    echo -e "${RED}Error: Invalid version format${NC}"
    echo "Version must be in format: X.Y.Z (e.g., 0.1.0)"
    exit 1
fi

# Check if we're on main branch
CURRENT_BRANCH=$(git branch --show-current)
if [ "$CURRENT_BRANCH" != "main" ]; then
    echo -e "${YELLOW}Warning: Not on main branch (current: $CURRENT_BRANCH)${NC}"
    read -p "Continue anyway? (y/N) " -n 1 -r
    echo
    if [[ ! $REPLY =~ ^[Yy]$ ]]; then
        exit 1
    fi
fi

# Check for uncommitted changes
if ! git diff-index --quiet HEAD --; then
    echo -e "${RED}Error: Uncommitted changes detected${NC}"
    echo "Please commit or stash your changes first."
    exit 1
fi

# Check if tag already exists
if git tag -l "v$VERSION" | grep -q "v$VERSION"; then
    echo -e "${RED}Error: Tag v$VERSION already exists${NC}"
    exit 1
fi

echo -e "${GREEN}Preparing release v$VERSION${NC}"

# Update version in Cargo.toml
echo "Updating Cargo.toml..."
sed -i.bak "s/^version = \"[^\"]*\"/version = \"$VERSION\"/" Cargo.toml
rm -f Cargo.toml.bak

# Update version in package.json
echo "Updating package.json..."
node -e "const fs=require('fs'); const p='package.json'; const pkg=JSON.parse(fs.readFileSync(p,'utf8')); pkg.version=process.argv[1]; fs.writeFileSync(p, JSON.stringify(pkg, null, 2) + '\n');" "$VERSION"

# Force Cargo to update Cargo.lock version
echo "Updating Cargo.lock..."
cargo check

# Run cargo tests
echo "Running tests..."
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-features

echo -e "${GREEN}Tests passed!${NC}"

# Commit version bump
echo "Committing version bump..."
git add Cargo.toml Cargo.lock package.json
git commit -m "Release v$VERSION"

# Create tag
echo "Creating tag v$VERSION..."
git tag -a "v$VERSION" -m "Release v$VERSION"

# Push changes and tag
echo "Pushing to origin..."
git push origin main
git push origin "v$VERSION"

echo ""
echo -e "${GREEN}✓ Release v$VERSION created successfully!${NC}"
echo ""
echo "GitHub Actions will now:"
echo "  1. Run the full Rust check & test suite"
echo "  2. Build binaries for Linux and macOS"
echo "  3. Create the GitHub release"
echo ""
echo "Monitor the release at:"
echo "  https://github.com/iisi-2209026/mcp-cli/actions"
