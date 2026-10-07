# Acme release tooling
SHELL := /bin/bash
VERSION ?= $(shell cat VERSION)
DIST := dist
TARBALL := $(DIST)/acme-$(VERSION).tar.gz
CACHE_DIR ?= $$HOME/.cache/acme
ENV ?= staging

.PHONY: all dist deploy rollback cache clean

all: dist

dist: $(TARBALL)

$(TARBALL):
	mkdir -p $(DIST)
	tar --exclude='*.pyc' -czf $@ src/ VERSION
	@echo "built $@ ($$(du -h $@ | cut -f1))"

deploy: dist
	REGION=$${REGION:-us-east-1} ./scripts/deploy.sh $(ENV)

rollback:
	@test -n "$(PREVIOUS)" || { echo 'usage: make rollback PREVIOUS=x.y.z' >&2; exit 1; }
	ssh deploy@$(ENV).acme.example "sudo /opt/acme/bin/activate --version '$(PREVIOUS)'"

cache: dist
	mkdir -p $$HOME/.cache/acme && cp $(TARBALL) $$HOME/.cache/acme/

clean:
	rm -rf $(DIST) && find . -name '*.pyc' -delete 2>/dev/null || true
