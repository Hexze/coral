variable "REGISTRY" { default = "ghcr.io/hexze/coral" }
variable "SHA"      { default = "" }

group "default" {
  targets = ["coral-api", "coral-bot", "coral-admin", "coral-verify", "coral-postgres"]
}

target "_common" {
  context    = "."
  dockerfile = "Dockerfile"
  secret     = ["id=git_auth_token,env=GIT_AUTH_TOKEN"]
}

target "coral-api" {
  inherits = ["_common"]
  target   = "coral-api"
  tags     = compact(["${REGISTRY}/coral-api:latest", SHA != "" ? "${REGISTRY}/coral-api:${SHA}" : ""])
}

target "coral-bot" {
  inherits = ["_common"]
  target   = "coral-bot"
  tags     = compact(["${REGISTRY}/coral-bot:latest", SHA != "" ? "${REGISTRY}/coral-bot:${SHA}" : ""])
}

target "coral-admin" {
  inherits = ["_common"]
  target   = "coral-admin"
  tags     = compact(["${REGISTRY}/coral-admin:latest", SHA != "" ? "${REGISTRY}/coral-admin:${SHA}" : ""])
}

target "coral-verify" {
  inherits = ["_common"]
  target   = "coral-verify"
  tags     = compact(["${REGISTRY}/coral-verify:latest", SHA != "" ? "${REGISTRY}/coral-verify:${SHA}" : ""])
}

target "coral-postgres" {
  inherits = ["_common"]
  target   = "coral-postgres"
  tags     = compact(["${REGISTRY}/coral-postgres:latest", SHA != "" ? "${REGISTRY}/coral-postgres:${SHA}" : ""])
}
