#!/bin/bash
set -e

# --- Color Definitions ---
GREEN='\033[0;32m'
BLUE='\033[0;34m'
YELLOW='\033[1;33m'
RED='\033[0;31m'
NC='\033[0m'

DB_NAME="modern_format_boost"
OS_TYPE=$(uname -s | tr '[:upper:]' '[:lower:]')

echo -e "${BLUE}🐘 Modern Format Boost - Database Manager v0.1.0${NC}"
echo "--------------------------------------------------------"

command_exists() {
    command -v "$1" >/dev/null 2>&1
}

check_deps() {
    if ! command_exists psql; then
        echo -e "${RED}❌ PostgreSQL not found. Please run 'sh scripts/install_deps.sh' first.${NC}"
        exit 1
    fi
}

start_service() {
    echo -e "${YELLOW}🔄 Attempting to start PostgreSQL service...${NC}"
    if [[ "$OS_TYPE" == "darwin" ]]; then
        if command_exists brew; then
            # Find the first installed postgresql service
            pg_service=$(brew services list | awk '/^postgresql/ {print $1}' | head -n 1)
            if [[ -n "$pg_service" ]]; then
                echo "Starting service '$pg_service' via Homebrew..."
                brew services start "$pg_service" || true
            else
                echo -e "${YELLOW}⚠️  No PostgreSQL service found in 'brew services'. Trying to install default...${NC}"
                brew install postgresql && brew services start postgresql || true
            fi
        else
            echo "Homebrew not found. Trying pg_ctl..."
            pg_ctl start || true
        fi
    elif [[ "$OS_TYPE" == "linux" ]]; then
        if command_exists systemctl; then
            sudo systemctl start postgresql
        else
            sudo service postgresql start
        fi
    fi
    sleep 2
}

setup_db() {
    echo -e "${BLUE}🏗️  Setting up database: $DB_NAME${NC}"

    # Check if DB exists
    if psql -lqt | cut -d \| -f 1 | grep -qw "$DB_NAME"; then
        echo -e "${GREEN}✅ Database '$DB_NAME' already exists.${NC}"
    else
        echo "Creating database '$DB_NAME'..."
        createdb "$DB_NAME" || psql -c "CREATE DATABASE $DB_NAME;"
    fi

    echo "Ensuring pgvector extension is enabled..."
    psql -d "$DB_NAME" -c "CREATE EXTENSION IF NOT EXISTS vector;" || {
        echo -e "${RED}❌ Failed to create 'vector' extension. Is 'pgvector' installed?${NC}"
        echo "Try: brew install pgvector (macOS) or refer to your Linux distribution's pgvector package."
        exit 1
    }

    echo -e "${GREEN}✅ Database setup complete!${NC}"
}

get_status() {
    if pg_isready >/dev/null 2>&1; then
        echo -e "Status: ${GREEN}RUNNING${NC}"
        if psql -lqt | cut -d \| -f 1 | grep -qw "$DB_NAME"; then
            echo -e "Database: ${GREEN}$DB_NAME (Ready)${NC}"
        else
            echo -e "Database: ${YELLOW}$DB_NAME (Missing, run 'setup')${NC}"
        fi
    else
        echo -e "Status: ${RED}STOPPED${NC}"
    fi
}

case "$1" in
start)
    check_deps
    start_service
    get_status
    ;;
setup)
    check_deps
    start_service
    setup_db
    ;;
status)
    get_status
    ;;
*)
    echo "Usage: $0 {start|setup|status}"
    exit 1
    ;;
esac
