{{/*
Expand the name of the chart.
*/}}
{{- define "sniper-suite.name" -}}
{{- default .Chart.Name .Values.nameOverride | trunc 63 | trimSuffix "-" }}
{{- end }}

{{/*
Fully qualified app name (chart name + release, truncated to 63 chars).
*/}}
{{- define "sniper-suite.fullname" -}}
{{- if .Values.fullnameOverride }}
{{- .Values.fullnameOverride | trunc 63 | trimSuffix "-" }}
{{- else }}
{{- $name := default .Chart.Name .Values.nameOverride }}
{{- if contains $name .Release.Name }}
{{- .Release.Name | trunc 63 | trimSuffix "-" }}
{{- else }}
{{- printf "%s-%s" .Release.Name $name | trunc 63 | trimSuffix "-" }}
{{- end }}
{{- end }}
{{- end }}

{{/*
Chart label.
*/}}
{{- define "sniper-suite.chart" -}}
{{- printf "%s-%s" .Chart.Name .Chart.Version | replace "+" "_" | trunc 63 | trimSuffix "-" }}
{{- end }}

{{/*
Common labels applied to every object.
*/}}
{{- define "sniper-suite.labels" -}}
helm.sh/chart: {{ include "sniper-suite.chart" . }}
app.kubernetes.io/name: {{ include "sniper-suite.name" . }}
app.kubernetes.io/instance: {{ .Release.Name }}
app.kubernetes.io/version: {{ .Chart.AppVersion | quote }}
app.kubernetes.io/managed-by: {{ .Release.Service }}
{{- with .Values.commonLabels }}
{{ toYaml . }}
{{- end }}
{{- end }}

{{/*
Immutable container image reference: `repository@digest`, never a tag.

The digest is required (an empty value fails the render instead of silently
producing a `repository@` image string) and must look like a sha256 digest, so
a copy-pasted tag or a truncated digest cannot roll a cluster. Paired with
values.schema.json, which rejects `image.tag` outright: there is no path from
a mutable reference through this chart to a pod spec.
*/}}
{{- define "sniper-suite.image" -}}
{{- $digest := required "image.digest is required (pin an immutable sha256:<64 hex> digest; this chart takes no tag)" .Values.image.digest -}}
{{- if not (regexMatch "^sha256:[a-f0-9]{64}$" $digest) -}}
{{- fail (printf "image.digest %q is not a sha256 digest — expected sha256: followed by 64 lowercase hex characters" $digest) -}}
{{- end -}}
{{- printf "%s@%s" .Values.image.repository $digest -}}
{{- end -}}

{{/*
Selector labels (subset of labels, stable across upgrades).
*/}}
{{- define "sniper-suite.selectorLabels" -}}
app.kubernetes.io/name: {{ include "sniper-suite.name" . }}
app.kubernetes.io/instance: {{ .Release.Name }}
{{- end }}

{{/*
Name of the Secret holding connection strings and keys. Uses the chart
Secret when secrets.create, otherwise the caller-provided existingSecret.
*/}}
{{- define "sniper-suite.secretName" -}}
{{- if .Values.secrets.existingSecret }}
{{- .Values.secrets.existingSecret }}
{{- else }}
{{- include "sniper-suite.fullname" . }}
{{- end }}
{{- end }}

{{/*
Name of the ServiceAccount.
*/}}
{{- define "sniper-suite.serviceAccountName" -}}
{{- if .Values.serviceAccount.create }}
{{- default (include "sniper-suite.fullname" .) .Values.serviceAccount.name }}
{{- else }}
{{- default "default" .Values.serviceAccount.name }}
{{- end }}
{{- end }}
