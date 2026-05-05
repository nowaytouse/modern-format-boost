#!/bin/bash

# GitHub Actions Workflow Health Check Script
# This script helps identify common workflow issues before they cause failures

set -euo pipefail

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

# Function to print colored output
print_status() {
    local color=$1
    local message=$2
    echo -e "${color}${message}${NC}"
}

# Function to check workflow syntax
check_workflow_syntax() {
    print_status $BLUE "🔍 Checking workflow syntax..."
    
    local workflow_files=$(find .github/workflows -name "*.yml" -o -name "*.yaml" 2>/dev/null || true)
    
    if [[ -z "$workflow_files" ]]; then
        print_status $RED "❌ No workflow files found"
        return 1
    fi
    
    local syntax_errors=0
    for file in $workflow_files; do
        if command -v yamllint >/dev/null 2>&1; then
            if ! yamllint "$file" >/dev/null 2>&1; then
                print_status $RED "❌ YAML syntax error in $file"
                yamllint "$file"
                ((syntax_errors++))
            else
                print_status $GREEN "✅ $file syntax OK"
            fi
        else
            print_status $YELLOW "⚠️  yamllint not installed, skipping YAML syntax check"
        fi
    done
    
    return $syntax_errors
}

# Function to check action versions
check_action_versions() {
    print_status $BLUE "🔍 Checking action versions..."
    
    local outdated_actions=0
    local workflow_files=$(find .github/workflows -name "*.yml" -o -name "*.yaml" 2>/dev/null || true)
    
    for file in $workflow_files; do
        # Check for deprecated or old action versions
        if grep -q "actions/checkout@v1\|actions/checkout@v2\|actions/checkout@v3" "$file"; then
            print_status $YELLOW "⚠️  Old checkout action version in $file (should be v4)"
            ((outdated_actions++))
        fi
        
        if grep -q "actions/cache@v1\|actions/cache@v2\|actions/cache@v3" "$file"; then
            print_status $YELLOW "⚠️  Old cache action version in $file (should be v4)"
            ((outdated_actions++))
        fi
        
        if grep -q "actions/upload-artifact@v1\|actions/upload-artifact@v2\|actions/upload-artifact@v3" "$file"; then
            print_status $YELLOW "⚠️  Old upload-artifact action version in $file (should be v4)"
            ((outdated_actions++))
        fi
        
        if grep -q "actions/download-artifact@v1\|actions/download-artifact@v2\|actions/download-artifact@v3" "$file"; then
            print_status $YELLOW "⚠️  Old download-artifact action version in $file (should be v4)"
            ((outdated_actions++))
        fi
        
        # Check for Node.js version issues
        if ! grep -q "FORCE_JAVASCRIPT_ACTIONS_TO_NODE24" "$file"; then
            print_status $YELLOW "⚠️  Missing FORCE_JAVASCRIPT_ACTIONS_TO_NODE24 in $file"
            ((outdated_actions++))
        fi
    done
    
    if [[ $outdated_actions -eq 0 ]]; then
        print_status $GREEN "✅ All action versions look up to date"
    fi
    
    return $outdated_actions
}

# Function to check permissions
check_permissions() {
    print_status $BLUE "🔍 Checking workflow permissions..."
    
    local permission_issues=0
    local workflow_files=$(find .github/workflows -name "*.yml" -o -name "*.yaml" 2>/dev/null || true)
    
    for file in $workflow_files; do
        # Check for overly broad permissions
        if grep -q "permissions: read-all" "$file"; then
            print_status $YELLOW "⚠️  Using 'read-all' permissions in $file (consider specific permissions)"
            ((permission_issues++))
        fi
        
        # Check for missing permissions on workflows that need them
        if grep -q "contents: write\|packages: write" "$file"; then
            if ! grep -q "permissions:" "$file"; then
                print_status $YELLOW "⚠️  Workflow needs write permissions but permissions block missing in $file"
                ((permission_issues++))
            fi
        fi
    done
    
    if [[ $permission_issues -eq 0 ]]; then
        print_status $GREEN "✅ Permissions look good"
    fi
    
    return $permission_issues
}

# Function to check for common anti-patterns
check_anti_patterns() {
    print_status $BLUE "🔍 Checking for common anti-patterns..."
    
    local anti_patterns=0
    local workflow_files=$(find .github/workflows -name "*.yml" -o -name "*.yaml" 2>/dev/null || true)
    
    for file in $workflow_files; do
        # Check for missing timeouts
        if ! grep -q "timeout-minutes:" "$file"; then
            print_status $YELLOW "⚠️  Missing timeout-minutes in $file"
            ((anti_patterns++))
        fi
        
        # Check for missing concurrency control on long-running jobs
        if grep -q "schedule:" "$file" && ! grep -q "concurrency:" "$file"; then
            print_status $YELLOW "⚠️  Scheduled workflow $file missing concurrency control"
            ((anti_patterns++))
        fi
        
        # Check for hardcoded secrets
        if grep -E "(password|secret|token|key)\s*:\s*['\"][^'\"]*['\"]" "$file" >/dev/null 2>&1; then
            print_status $RED "❌ Potential hardcoded secret in $file"
            ((anti_patterns++))
        fi
        
        # Check for missing error handling
        if grep -q "curl\|wget\|download" "$file" && ! grep -q "|| true\|&&\|continue-on-error" "$file"; then
            print_status $YELLOW "⚠️  Network operations in $file may need error handling"
            ((anti_patterns++))
        fi
    done
    
    if [[ $anti_patterns -eq 0 ]]; then
        print_status $GREEN "✅ No common anti-patterns found"
    fi
    
    return $anti_patterns
}

# Function to check Rust-specific issues
check_rust_specific() {
    print_status $BLUE "🔍 Checking Rust-specific issues..."
    
    local rust_issues=0
    
    # Check if rust-toolchain.toml exists and is valid
    if [[ -f "rust-toolchain.toml" ]]; then
        if ! grep -q "channel\s*=" rust-toolchain.toml; then
            print_status $YELLOW "⚠️  rust-toolchain.toml missing channel specification"
            ((rust_issues++))
        else
            print_status $GREEN "✅ rust-toolchain.toml looks good"
        fi
    else
        print_status $YELLOW "⚠️  No rust-toolchain.toml found (recommended for consistent builds)"
        ((rust_issues++))
    fi
    
    # Check for consistent Rust toolchain usage in workflows
    local workflow_files=$(find .github/workflows -name "*.yml" -o -name "*.yaml" 2>/dev/null || true)
    local toolchain_inconsistency=0
    
    for file in $workflow_files; do
        if grep -q "dtolnay/rust-toolchain" "$file"; then
            if grep -q "channel.*nightly" "$file" && ! grep -q "nightly" <<< "$(basename "$file")"; then
                print_status $YELLOW "⚠️  Using nightly Rust in $file (consider stable for reliability)"
                ((toolchain_inconsistency++))
            fi
        fi
    done
    
    if [[ $toolchain_inconsistency -eq 0 ]]; then
        print_status $GREEN "✅ Rust toolchain usage looks consistent"
    fi
    
    return $((rust_issues + toolchain_inconsistency))
}

# Main execution
main() {
    print_status $BLUE "🚀 Starting GitHub Actions workflow health check..."
    echo
    
    local total_issues=0
    
    check_workflow_syntax
    ((total_issues += $?))
    echo
    
    check_action_versions
    ((total_issues += $?))
    echo
    
    check_permissions
    ((total_issues += $?))
    echo
    
    check_anti_patterns
    ((total_issues += $?))
    echo
    
    check_rust_specific
    ((total_issues += $?))
    echo
    
    # Summary
    if [[ $total_issues -eq 0 ]]; then
        print_status $GREEN "🎉 All checks passed! Your workflows look healthy."
    else
        print_status $YELLOW "⚠️  Found $total_issues potential issues to review."
        print_status $BLUE "💡 Consider addressing these issues to improve workflow reliability."
    fi
    
    return $total_issues
}

# Run main function
main "$@"
