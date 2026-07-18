class Admin::AppClientsController < Admin::BaseController
  def index
    @app_clients = AppClient.includes(:client_department_accesses)
                            .order(:client_identifier)
    # Resolve departments via with_deleted: a soft-deleted (trashed) department
    # is hidden by Department's default_scope, so access.department would read
    # nil and the view would 500 on .name. The access rows outlive soft-delete.
    dept_ids = @app_clients.flat_map { |c| c.client_department_accesses.map(&:department_id) }
    @departments_by_id = Department.with_deleted.where(id: dept_ids).index_by(&:id)
  end

  def new
    @app_client = AppClient.new
    @departments = departments_for_form(@app_client)
  end

  def edit
    @app_client = AppClient.find(params[:id])
    @departments = departments_for_form(@app_client)
  end

  def create
    @app_client = AppClient.new(app_client_params)
    if save_with_access
      redirect_to admin_app_clients_path, notice: "Client registered."
    else
      flash.now[:alert] = @app_client.errors.full_messages.join(", ")
      @departments = departments_for_form(@app_client)
      render :new, status: :unprocessable_entity
    end
  end

  def update
    @app_client = AppClient.find(params[:id])
    @app_client.assign_attributes(app_client_params)
    if save_with_access
      redirect_to admin_app_clients_path, notice: "Client updated."
    else
      flash.now[:alert] = @app_client.errors.full_messages.join(", ")
      @departments = departments_for_form(@app_client)
      render :edit, status: :unprocessable_entity
    end
  end

  def destroy
    AppClient.find(params[:id]).destroy
    redirect_to admin_app_clients_path, notice: "Client deleted."
  end

  private

  def app_client_params
    params.require(:app_client).permit(:client_identifier, :description)
  end

  # Live departments plus any (trashed) department the client already has a
  # grant for, so an existing access to a soft-deleted department stays visible
  # and removable in the grid instead of silently persisting off-screen.
  def departments_for_form(client)
    ids = Department.pluck(:id)
    ids += client.client_department_accesses.pluck(:department_id) if client.persisted?
    Department.with_deleted.where(id: ids.uniq).order(:name)
  end

  def save_with_access
    AppClient.transaction do
      @app_client.save!
      sync_access!(@app_client, access_params)
    end
    true
  rescue ActiveRecord::RecordInvalid
    false
  end

  # Grid params: app_client[access][<department_id>][read|write] = "1"/"0"
  # (checkboxes ship a hidden "0", so every rendered department reports both
  # flags). Unknown department ids are dropped by the DB lookup in sync_access!.
  def access_params
    raw = params[:app_client][:access]
    return {} unless raw.is_a?(ActionController::Parameters)

    raw.to_unsafe_h.transform_values do |flags|
      flags = {} unless flags.is_a?(Hash)
      { can_read: flags["read"] == "1", can_write: flags["write"] == "1" }
    end
  end

  # Reconciles the submitted grid into client_department_accesses rows:
  # read and/or write checked upserts the row, fully unchecked removes it.
  # Looks up with_deleted so a grant to a trashed department (rendered by
  # departments_for_form) can be removed; the loaded department is assigned to
  # the row so the belongs_to presence validation passes on upsert even when
  # Department's default_scope would otherwise read it back as nil.
  def sync_access!(client, desired)
    Department.with_deleted.where(id: desired.keys).find_each do |department|
      flags = desired[department.id.to_s]
      row = client.client_department_accesses
                  .find_or_initialize_by(department_id: department.id)
      row.department = department
      if flags[:can_read] || flags[:can_write]
        row.update!(flags)
      elsif row.persisted?
        row.destroy!
      end
    end
  end
end
