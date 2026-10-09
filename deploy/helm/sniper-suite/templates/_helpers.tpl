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
